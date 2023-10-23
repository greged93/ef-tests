import os
import re


def main():
    # Get environment variables
    output = os.getenv("OUTPUT")
    print(output)
    nums = re.findall("hello world ([0-9])", output)
    print(nums)


if __name__ == "__main__":
    main()
