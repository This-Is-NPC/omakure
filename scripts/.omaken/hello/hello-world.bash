#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {
#   "Name": "hello-world",
#   "Description": "Prints hello once every minute (scheduler smoke test).",
#   "Tags": ["demo", "scheduler"],
#   "Fields": [
#     {
#       "Name": "greeting",
#       "Prompt": "Greeting text",
#       "Type": "string",
#       "Arg": "--greeting",
#       "Default": "hello world!",
#       "Required": false
#     }
#   ],
#   "Schedule": {
#     "Cron": "*/10 * * * * *",
#     "Enabled": true
#   }
# }
# OMAKURE_SCHEMA_END

set -euo pipefail

greeting="hello world!"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --greeting)
      greeting="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

echo "[$(date -Iseconds)] ${greeting}"
