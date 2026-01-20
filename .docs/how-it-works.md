# How it works (overview)

1) Scripts live anywhere under `~/Documents/omakure-scripts` (Windows: `%USERPROFILE%\Documents\omakure-scripts`) with `.bash`, `.sh`, `.ps1`, or `.py` extensions.
2) Scripts embed their schema as a commented JSON block between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`.
3) If a folder has `index.lua`, the TUI renders the widget in the header panel. See `lua-widgets.md`.
4) The TUI reads schemas, shows Outputs/Queue details when present, prompts for values, and runs the script with args.
5) Every execution is captured in `.history/`.

## Script index (examples)

| Script | Description |
| --- | --- |
| `scripts/.omaken/azure/rg-list-all.bash` | List resource groups with CreatedAt, LastModified, and CreatedBy. |
| `scripts/.omaken/azure/rg-details.bash` | Show resource group details and list resources with CreatedAt, LastModified, and CreatedBy. |
| `scripts/.omaken/azure/rg-delete.bash` | Delete a resource group and all resources inside it. |
