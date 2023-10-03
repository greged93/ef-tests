use blockifier::abi::abi_utils::{
    get_erc20_balance_var_addresses, get_storage_var_address, get_uint256_storage_var_addresses,
};
use blockifier::state::state_api::{State, StateResult};
use reth_primitives::{Address, Bytes};
use revm_primitives::U256;
use starknet_api::core::Nonce;
use starknet_api::hash::StarkFelt;
use starknet_api::StarknetApiError;

use super::constants::{
    CONTRACT_ACCOUNT_CLASS_HASH, EOA_CLASS_HASH, FEE_TOKEN_ADDRESS, KAKAROT_ADDRESS,
    PROXY_CLASS_HASH,
};
use super::types::FeltSequencer;
use super::utils::{compute_starknet_address, split_bytecode_to_starkfelt, split_u256};
use super::KakarotSequencer;

pub trait EvmState {
    fn setup_account(
        &mut self,
        evm_address: &Address,
        bytecode: &Bytes,
        nonce: U256,
        storage: Vec<(U256, U256)>,
    ) -> StateResult<()>;

    fn fund(&mut self, evm_address: &Address, balance: U256) -> StateResult<()>;
}

impl EvmState for KakarotSequencer {
    fn setup_account(
        &mut self,
        evm_address: &Address,
        bytecode: &Bytes,
        nonce: U256,
        evm_storage: Vec<(U256, U256)>,
    ) -> StateResult<()> {
        let nonce = StarkFelt::from(TryInto::<u128>::try_into(nonce).map_err(|err| {
            StarknetApiError::OutOfRange {
                string: err.to_string(),
            }
        })?);
        let starknet_address = compute_starknet_address(evm_address);
        let evm_address = Into::<FeltSequencer>::into(*evm_address).into();

        let mut storage = vec![
            (("evm_address", vec![]), evm_address),
            (("is_initialized_", vec![]), StarkFelt::from(1u8)),
            (("Ownable_owner", vec![]), *KAKAROT_ADDRESS.0.key()),
            (
                ("bytecode_len_", vec![]),
                StarkFelt::from(bytecode.len() as u32),
            ),
            (("kakarot_address", vec![]), *KAKAROT_ADDRESS.0.key()),
        ];

        let starknet_address = starknet_address.try_into()?;
        // Initialize the implementation and nonce based on account type.
        if bytecode.is_empty() && evm_storage.is_empty() {
            storage.push((("_implementation", vec![]), EOA_CLASS_HASH.0));
            self.0.state.set_nonce(starknet_address, Nonce(nonce));
        } else {
            storage.append(&mut vec![
                (("nonce", vec![]), nonce),
                (("_implementation", vec![]), CONTRACT_ACCOUNT_CLASS_HASH.0),
            ]);
        }

        // Initialize the bytecode storage var.
        let bytecode_storage = &mut split_bytecode_to_starkfelt(bytecode)
            .into_iter()
            .enumerate()
            .map(|(i, bytes)| (("bytecode_", vec![StarkFelt::from(i as u32)]), bytes))
            .collect();
        storage.append(bytecode_storage);

        // Initialize the storage vars.
        let evm_storage_storage = &mut evm_storage
            .iter()
            .flat_map(|(k, v)| {
                let keys = split_u256(*k).map(Into::into);
                let values = split_u256(*v).map(Into::into);
                vec![
                    (("storage_", vec![keys[0]]), values[0]),
                    (("storage_", vec![keys[1]]), values[1]),
                ]
            })
            .collect();
        storage.append(evm_storage_storage);

        // Write all the storage vars to the sequencer state.
        for ((var, keys), v) in storage {
            (&mut self.0.state).set_storage_at(
                starknet_address,
                get_storage_var_address(var, &keys).unwrap(), // safe unwrap: all vars are ASCII
                v,
            );
        }

        // Set up the contract class hash.
        (&mut self.0.state).set_class_hash_at(starknet_address, *PROXY_CLASS_HASH)?;

        // Add the address to the Kakarot evm to starknet mapping
        let evm_starknet_address_mapping_storage = (
            get_storage_var_address("evm_to_starknet_address", &[evm_address]).unwrap(), // safe unwrap: var is ASCII
            *starknet_address.0.key(),
        );
        (&mut self.0.state).set_storage_at(
            *KAKAROT_ADDRESS,
            evm_starknet_address_mapping_storage.0,
            evm_starknet_address_mapping_storage.1,
        );
        Ok(())
    }

    fn fund(&mut self, evm_address: &Address, balance: U256) -> StateResult<()> {
        let starknet_address = compute_starknet_address(evm_address);
        let balance_values = split_u256(balance);
        let mut storage = vec![];

        // Initialize the balance storage var.
        let balance_keys = get_erc20_balance_var_addresses(&FEE_TOKEN_ADDRESS)?;
        let balance_keys = [balance_keys.0, balance_keys.1];
        let balance_storage = &mut balance_keys
            .into_iter()
            .zip(balance_values)
            .map(|(k, v)| (k, StarkFelt::from(v)))
            .collect();
        storage.append(balance_storage);

        // Initialize the allowance storage var.
        let allowance_keys = get_uint256_storage_var_addresses(
            "ERC20_allowances",
            &[*FEE_TOKEN_ADDRESS.0.key(), starknet_address.into()],
        )?;
        let allowance_keys = [allowance_keys.0, allowance_keys.1];
        let allowance_storage = &mut allowance_keys
            .into_iter()
            .map(|k| (k, StarkFelt::from(u128::MAX)))
            .collect();
        storage.append(allowance_storage);

        // Write all the storage vars to the sequencer state.
        let starknet_address = starknet_address.try_into()?;
        for (k, v) in storage {
            (&mut self.0.state).set_storage_at(starknet_address, k, v);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::evm_sequencer::{
        constants::{
            tests::{PRIVATE_KEY, PUBLIC_KEY, TEST_CONTRACT_ADDRESS},
            CHAIN_ID,
        },
        utils::to_broadcasted_starknet_transaction,
    };

    use super::*;
    use blockifier::state::state_api::StateReader;
    use bytes::BytesMut;
    use reth_primitives::{sign_message, AccessList, Signature, TransactionSigned, TxEip1559};
    use revm_primitives::B256;
    use sequencer::{
        execution::Execution, state::State as SequencerState, transaction::StarknetTransaction,
    };
    use starknet::core::types::{BroadcastedTransaction, FieldElement};

    #[test]
    fn test_execute_simple_contract() {
        // Given
        let sequencer = KakarotSequencer::new(SequencerState::default());
        let mut sequencer = sequencer.initialize().unwrap();

        let transaction = TransactionSigned {
            hash: B256::default(),
            signature: Signature::default(),
            transaction: reth_primitives::Transaction::Eip1559(TxEip1559 {
                chain_id: *CHAIN_ID,
                nonce: 0,
                gas_limit: 0,
                max_fee_per_gas: 0,
                max_priority_fee_per_gas: 0,
                to: reth_primitives::TransactionKind::Call(*TEST_CONTRACT_ADDRESS),
                value: 0,
                access_list: AccessList::default(),
                input: Bytes::default(),
            }),
        };
        let signature =
            sign_message(*PRIVATE_KEY, transaction.transaction.signature_hash()).unwrap();
        let mut output = BytesMut::new();
        transaction.encode_with_signature(&signature, &mut output, false);
        let transaction = BroadcastedTransaction::Invoke(
            to_broadcasted_starknet_transaction(&output.to_vec().into()).unwrap(),
        );
        let transaction = StarknetTransaction::new(transaction)
            .try_into_execution_transaction(FieldElement::from(*CHAIN_ID))
            .unwrap();

        // When
        let bytecode = Bytes::from(vec![96, 1, 96, 0, 85]); // PUSH 01 PUSH 00 SSTORE
        let nonce = U256::from(0);
        sequencer
            .setup_account(&TEST_CONTRACT_ADDRESS, &bytecode, nonce, vec![])
            .unwrap();
        sequencer
            .setup_account(&PUBLIC_KEY, &Bytes::default(), U256::from(0), vec![])
            .unwrap();
        sequencer.0.execute(transaction).unwrap();

        // Then
        let contract_starknet_address = compute_starknet_address(&TEST_CONTRACT_ADDRESS)
            .try_into()
            .unwrap();
        let storage = (&mut sequencer.0.state)
            .get_storage_at(
                contract_starknet_address,
                get_storage_var_address("storage_", &[StarkFelt::from(0u8), StarkFelt::from(0u8)])
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(storage, StarkFelt::from(1u8));
    }
}
