# Scripts path

The workspace is the scripts root and the owner of Omakure metadata.

## Resolution precedence

The first applicable entry wins:

1. `--scripts-dir <PATH>`.
2. `OMAKURE_SCRIPTS_DIR`.
3. Repository `scripts/workspace/` in debug builds, when present.
4. `~/Documents/omakure-scripts` or the Windows Documents equivalent.

The workspace root cannot be selected with a bare positional path:
`omakure PATH` is a command-line error. Commands such as `run` and `describe`
accept script names or paths within the selected workspace. Scripts, metadata,
history, environments, and the search index share that root.

## Repository automation

The top-level `scripts/` hierarchy is reserved for repository automation:
`scripts/tasks/` contains mise file tasks and `scripts/fixtures/` contains
certification support. It is not a subject-script collection. Subject scripts
come from external Battery repositories and are explicitly installed into the
selected workspace.


## Examples

```bash
omakure --scripts-dir /srv/omakure-scripts --json scripts
OMAKURE_SCRIPTS_DIR=/srv/omakure-scripts omakure doctor
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
