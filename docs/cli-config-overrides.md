# CLI `--config` Overrides Reference

This document describes how to override rumdl configuration directly on the
command line, without modifying any config file. This mirrors
[Ruff's `--config`](https://docs.astral.sh/ruff/configuration/#config-file-discovery)
flag overload.

## Overview

The `--config` flag accepts either:

1. A **path** to a TOML configuration file (`.rumdl.toml`, `rumdl.toml`,
   `pyproject.toml`, …), or
2. An **inline TOML `KEY = VALUE` snippet** that overrides specific options at
   the highest precedence — beating values from config files and from other CLI
   flags such as `--disable` or `--line-length`.

Both forms can be mixed; `--config` may be passed multiple times. At most one
value may be a file path; the remaining values must be inline TOML.

## Quick Examples

```bash
# Override a rule option without editing config
rumdl check --config 'MD013.line-length=120'

# Combine multiple overrides — each --config is independent, not replacement
rumdl check --config 'MD013.line-length=120' --config 'MD013.reflow=true'

# Override a top-level (global) setting
rumdl check --config 'line-length=120'

# Use the explicit [global] section name
rumdl check --config 'global.flavor="mkdocs"'

# Pass arrays
rumdl check --config 'disable=["MD013","MD033"]'
rumdl check --config 'exclude=["**/tmp/*.md"]'

# A file plus inline overrides — inline always wins
rumdl check --config my-config.toml --config 'MD013.line-length=120'

# Inline overrides remain in effect with --no-config / --isolated
rumdl check --no-config --config 'MD013.reflow=true'
```

## Syntax Forms

| Form                                | Where it lands                   | Example                                |
| ----------------------------------- | -------------------------------- | -------------------------------------- |
| `RULE.option = value`               | Per-rule option                  | `--config 'MD013.line-length=120'`     |
| `option = value` (top-level)        | Global option                    | `--config 'line-length=120'`           |
| `global.option = value`             | Global option (explicit section) | `--config 'global.flavor="mkdocs"'`    |

The dispatch is based on the **shape** of the value, not the key:

- A **table value** (e.g. `MD013.line-length=20` parses as a nested table) is
  treated as a per-rule override. The key is resolved against the rule registry,
  including markdownlint aliases (`line-length` → `MD013`).
- A **scalar or array** is treated as a top-level/global override. This matters
  for keys like `line-length`, which is both a global setting and an MD013
  alias: a bare `line-length=120` is global; `MD013.line-length=120` (or
  `line-length.line-length=120`) is per-rule.

## Precedence

Inline `--config` overrides are applied at `ConfigSource::Cli` precedence — the
highest. Order from lowest to highest:

1. Built-in defaults
2. User configuration (`~/.config/rumdl/rumdl.toml`)
3. `pyproject.toml`
4. Project configuration (`.rumdl.toml` / `rumdl.toml`)
5. **Other CLI flags** (`--disable`, `--line-length`, …)
6. **`--config` inline overrides** ← wins

When multiple `--config` arguments target the same key, the **last** one wins.

## Key Resolution

Both rule names and option keys are normalized:

- Rule names accept aliases: `--config 'line-length.line-length=20'` is the same
  as `--config 'MD013.line-length=20'`. Lowercase IDs (`md013`) also work.
- Option keys accept kebab-case and snake_case interchangeably:
  `MD013.line_length=20` is equivalent to `MD013.line-length=20`. The
  implementation collapses kebab/snake variants so `serde` does not see
  duplicate fields when an alias is set in a config file.

## Path Detection

A `--config` value with no `=` character is always treated as a file path. A
value containing `=` is preferred as a file path **only** if a file by that
literal name exists; otherwise it is parsed as inline TOML.

If a `--config` value has `=` and is neither an existing file nor valid TOML,
clap reports a `ValueValidation` error with the parser's TOML error message and
a usage tip — no panic, no silent fallback.

## Validation Warnings

The standard config validator runs against the merged config, so inline
`--config` overrides surface the same warnings as values from a config file:

| Bad input                              | Warning                                                    |
| -------------------------------------- | ---------------------------------------------------------- |
| `--config 'MD9999.foo=1'`              | `Unknown rule in config: MD9999`                           |
| `--config 'MD013.no_such_option=1'`    | `Unknown option for rule MD013: no_such_option`            |
| `--config 'totally_bogus_key=1'`       | `Unknown global option: totally_bogus_key`                 |
| `--config 'line-length="huge"'`        | `[--config] expected integer for global key 'line-length'` |

Warnings are written to stderr and do not affect the exit code on their own.
The lint/format command continues with the valid portions of the merged
config.

## Errors

| Bad input                                 | Outcome                                                                                                                   |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| Two `--config` values that are file paths | Tool error: `multiple --config file paths given`                                                                          |
| `--config /no/such/file.toml`             | Tool error: `config file not found`                                                                                       |
| `--config /file.toml --no-config`         | Tool error: `--config <CONFIG_OPTION> (file path) cannot be used with --no-config` (inline TOML overrides are unaffected) |
| `--config 'this is not valid toml = ='`   | clap value error with TOML parse details                                                                                  |

## Watch Mode

Inline overrides are reapplied on every config-file reload while watching, so
edits to `.rumdl.toml` cannot quietly undo what was set on the command line.

## Related References

- [Configuration File Format](global-settings.md) — full list of global settings.
- [Inline Configuration](inline-configuration.md) — `<!-- rumdl-disable -->`
  directives inside Markdown files (a different feature with a similar name).
