# Environment documents

Environment defaults live in `.omaken/envs/*.conf`. The active file name is stored in `.omaken/envs/active`.

## How it works

- Each line is `KEY=value`.
- Keys are matched (case-insensitive) to schema field names.
- When a match exists, the value is used as the default in the TUI.

## Switch environments

Use the TUI (Alt+E) to select the active file.

## Environments UI

The Environments screen shows a preview panel on the right for the selected file.
The preview lists parsed KEY=VALUE entries and masks sensitive values with `***`.

Preview scroll shortcuts:

- `PgUp` / `PgDn`
- `Home` / `End`

## Example

```
SUBSCRIPTION_ID=00000000-0000-0000-0000-000000000000
RESOURCE_GROUP=rg-prod
REGION=eastus
```

Preview example:

```
SUBSCRIPTION_ID=***
RESOURCE_GROUP=rg-prod
REGION=eastus
```

## Start from the template

Copy `.omaken/envs/env_template.conf` to a new `.conf` file and edit the values.
