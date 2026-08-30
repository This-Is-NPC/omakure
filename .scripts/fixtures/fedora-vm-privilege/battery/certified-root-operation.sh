#!/usr/bin/env bash
set -euo pipefail

# OMAKURE_SCHEMA_START
# {"Name":"Certified root operation","Description":"Run the one root operation authorized by the host policy","Tags":["certification","privilege"],"Fields":[]}
# OMAKURE_SCHEMA_END

systemctl --no-ask-password start omakure-certified-root-operation.service
