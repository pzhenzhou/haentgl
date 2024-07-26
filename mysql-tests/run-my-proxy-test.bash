#!/bin/bash
export BACKEND_SOCK=@mysqld.1.socket
export BACKEND_PORT=3310

test_help() {
   echo "Usage: $0 [--help] db_user [db_password] db_port [case_name]"
   echo "Run tests with the specified parameters."
   echo "Parameters:"
   echo "  --help       Display this help message."
   echo "  db_user      Database username."
   echo "  db_password  (Optional) Database password (can be null)."
   echo "  db_port      Database port (must be a non-empty number with a maximum value of 65535)."
   echo "  case_name    (Optional) Test case name. If not provided runs all test suites."
   echo "Example:"
   echo "  $0 db_user db_password 3310 insert_select.test"
}

is_valid_case_name() {
    # Parameter:
    # $1: case_name - Test case name
    local case_name="$1"  # Local variable to store the parameter
    local valid_cases=("connect.test" "insert_select.test" "change_user.test", "insert_update.test" "select.test" "trigger.test" "view.test")

    # Check if case_name is in the array of valid cases
    for valid_case in "${valid_cases[@]}"; do
        if [ "$valid_case" == "$case_name" ]; then
            return 0
        fi
    done
    return 1
}

run_test () {
    # Check for --help option
    # Parameters:
    # $1: db_user - Database username
    # $2: db_password - Database password
    # $3: db_port - Database port
    # $4: case_name - Test case name
    if [ "$1" == "--help" ]; then
      test_help
      exit 0
    fi

    db_user="$1"
    db_password="$2"
    db_port="$3"
    case_name="$4"
    mtr_cmd_template="./mysql-test-run.pl --suite my_proxy --fast --verbose --extern protocol=TCP --extern host=127.0.0.1"
    # Validate db_user
    if [ -z "$db_user" ]; then
        echo "Error: db_user cannot be null."
        test_help
        exit 1
    fi
    mtr_cmd_template=$mtr_cmd_template" --extern user=$db_user"

    if [ -n "$db_password" ]; then
       mtr_cmd_template=$mtr_cmd_template" --extern password=$db_password"
    fi

    # Validate db_port
    if [ -z "$db_port" ] || ! [[ "$db_port" =~ ^[0-9]+$ ]] || [ "$db_port" -gt 65535 ]; then
        echo "Error: db_port must be a non-empty number with a maximum value of 65535."
        test_help
        exit 1
    fi
    mtr_cmd=$mtr_cmd_template" --extern port=$db_port"

    if [ -n "$case_name" ]; then
      if ! is_valid_case_name "$case_name"; then
         echo "Error: Invalid case_name. It must be one of: ${valid_cases[*]}"
         test_help
         exit 1
      fi
      mtr_cmd=$mtr_cmd" my_proxy/$case_name"
    fi
    echo "$mtr_cmd"
    if ! eval "$mtr_cmd"; then
      echo "Error: Failed to execute mysql-test-run.pl."
      exit 1
    fi
}

run_test "$@"
