# Scripts path

The workspace is the scripts root and the owner of Omakure metadata.

## Resolution precedence

The first applicable entry wins:

1. `--scripts-dir <PATH>`.
2. `OMAKURE_SCRIPTS_DIR`.
3. `OVERTURE_SCRIPTS_DIR` (legacy).
4. `CLOUD_MGMT_SCRIPTS_DIR` (legacy).
5. Repository `scripts/` in debug builds, when present.
6. `~/Documents/omakure-scripts` or the Windows Documents equivalent.
7. Legacy `overture-scripts` or `cloud-mgmt-scripts` directories, when present.
8. The default Omakure directory as a first-launch fallback.

There is no positional path mode. `omakure PATH` is not a supported alias for
`--scripts-dir` and should be treated as a command-line error. This keeps
scripts, metadata, history, environments, and the search index under one
explicit root.

## Examples

```bash
omakure --scripts-dir /srv/omakure-scripts --json scripts
OMAKURE_SCRIPTS_DIR=/srv/omakure-scripts omakure --json doctor
```

On Windows, the Documents directory is resolved through the registry before
the `%USERPROFILE%\Documents` fallback.

## Ignore files

Create `.omakureignore` at the root or in a child directory to exclude helpers,
fixtures, generated files, or vendored folders:

```gitignore
helpers/
fixtures/*.sh
*.tmp.py
scratch.py
```

Blank lines and `#` comments are ignored. Patterns are relative to the file;
leading `/` anchors a pattern, trailing `/` prunes a directory, `*` matches a
sequence, and patterns without `/` match any path component. Nested ignore
files combine with parent rules. Negation, special `**`, character classes,
and escaped comments are not implemented. Unreadable ignore files produce a
warning and scanning continues with built-in `.history`, `.git`, and `.omakure`
exclusions.
