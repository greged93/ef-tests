use regex::Regex;
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::PathBuf};

use crate::dir_reader::PathWrapper;

/// Filter to be applied on the tests files
#[derive(Deserialize)]
pub struct Filter {
    /// Mapping containing the directories and the files that should be skipped
    filename: BTreeMap<String, Vec<String>>,
    /// Mapping containing the directories and the regex patterns that should be skipped
    regex: BTreeMap<String, Vec<String>>,
}

impl Filter {
    pub fn new(path: PathBuf) -> Self {
        let skip_str = fs::read_to_string(path).expect("Error while reading the skip file");
        serde_yaml::from_str(&skip_str).expect("Error while deserializing into Filter struct")
    }

    /// Checks if the given path should be skipped
    pub fn is_skipped(&self, path: &PathWrapper) -> bool {
        let dir_name = path.parent().file_stem_to_string();
        let file_name = path.file_stem_to_string();

        let mut should_skip = if self.filename.contains_key(&dir_name) {
            self.filename
                .get(&dir_name)
                .unwrap() // safe unwrap
                .iter()
                .any(|filename| filename == &file_name)
        } else {
            false
        };

        if !should_skip && self.regex.contains_key(&dir_name) {
            should_skip = self.regex.get(&dir_name).unwrap().iter().any(|regex| {
                Regex::new(regex.as_str())
                    .expect("Error with regex pattern")
                    .is_match(&file_name)
            });
        }
        should_skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_filter_file() {
        let filter = Filter::new(Path::new("../../blockchain-tests-skip.yml").to_path_buf());
        let path = PathWrapper::from(Path::new(
            "../../ef-testing/ethereum-tests/BlockchainTests/GeneralStateTests/stCallCreateCallCodeTest/Call1024PreCalls.json",
        ).to_path_buf());
        assert!(filter.is_skipped(&path));
    }

    #[test]
    fn test_filter_regex() {
        let filter = Filter::new(Path::new("../../blockchain-tests-skip.yml").to_path_buf());
        let path = PathWrapper::from(Path::new(
            "../../ef-testing/ethereum-tests/BlockchainTests/GeneralStateTests/stBadOpcode/opc4DDiffPlaces.json",
        ).to_path_buf());
        assert!(filter.is_skipped(&path));
    }
}