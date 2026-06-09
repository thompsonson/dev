# ADR 002: TOML config and session URI addressing

## Status

Draft

## Context

The current config format is a hand-parsed INI-style file at `~/.config/dev/config`:

```ini
default_layout=claude
default_host=pop-mini
atomicguard=claude@pop-mini
dotfiles=claude:~/.local/share/chezmoi
```

This format has served well for simple cases but has two hard limits:

1. **Flat structure.** A project entry is a single line: `layout[:path][@host]`. There is no way to express per-worktree config — layout overrides, custom paths, or host bindings for a worktree branch. Adding worktrees to the INI format would require either a new delimiter convention or a separate file, both of which are worse than the problem.

2. **No session addressing beyond project names.** `dev atomicguard` works because the project name is unambiguous. Once worktrees exist, `dev atomicguard/fix-guards` must resolve to a specific worktree of a specific project. The config needs to express that hierarchy before the CLI can parse it reliably.

The session URI design (below) requires the config to carry hierarchy. TOML expresses that hierarchy natively without a custom parser.

## Decision

### 1. Config format: INI → TOML

Replace the hand-parsed INI config with a TOML file at `~/.config/dev/config.toml`.

The existing INI parser and `parse_config_str` are removed. The `toml = "0.8"` crate with serde derives replaces them. All current config patterns map without data loss:

| Current INI | TOML equivalent |
|---|---|
| `default_layout=claude` | `[defaults]` `layout = "claude"` |
| `default_host=pop-mini` | `[defaults]` `host = "pop-mini"` |
| `myproject=claude` | `[project.myproject]` `layout = "claude"` |
| `dotfiles=claude:~/.local/share/chezmoi` | `[project.dotfiles]` `layout = "claude"` `path = "~/.local/share/chezmoi"` |
| `remote=default@server1` | `[project.remote]` `layout = "default"` `host = "server1"` |

`~` expansion in `path` values is handled in the config loading layer after deserialization, not in serde — same as today.

Project keys containing `/` (collision-disambiguated names such as `org/shared`) are valid TOML quoted keys (`["project"."org/shared"]`) and deserialize correctly into `HashMap<String, ProjectEntry>`.

### 2. Schema

The expected authoring style for a project with worktrees is TOML subtable syntax — `[project.x.worktree.y]` extends the `[project.x]` table. A complete example:

```toml
[defaults]
layout = "claude"       # applied when no per-project layout is set
host   = "pop-mini"     # applied when no per-project host is set

[project.atomicguard]
layout = "claude"
host   = "pop-mini"

[project.atomicguard.worktree.fix-guards]
layout = "default"      # optional: overrides project layout
                        # path: auto-derived from git worktree list
                        # host: inherits from project if absent

[project.dotfiles]
layout = "claude"
path   = "~/.local/share/chezmoi"
```

`[project.atomicguard]` and `[project.atomicguard.worktree.fix-guards]` coexist correctly — `toml 0.8` deserializes subtables into nested `HashMap` entries as expected. Projects without worktrees (such as `[project.dotfiles]` above) deserialize cleanly because `worktree` carries `#[serde(default)]`.

The Rust structs:

```rust
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct DevConfig {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub project: HashMap<String, ProjectEntry>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct Defaults {
    pub layout: Option<Layout>,
    pub host: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProjectEntry {
    pub layout: Option<Layout>,
    pub path: Option<PathBuf>,
    pub host: Option<String>,
    #[serde(default)]
    pub worktree: HashMap<String, WorktreeEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorktreeEntry {
    pub layout: Option<Layout>,     // None = inherit from project
    pub path: Option<PathBuf>,      // None = derived from git worktree list
    pub host: Option<String>,       // None = inherit from project
}
```

`None`-means-inherit throughout — no field is repeated when the parent default applies.

### 3. Session URI grammar

A session is addressed by a URI of the form:

```
[host/]project[/worktree]
```

Examples:

```
atomicguard                     local project, main worktree
atomicguard/fix-guards          local project, named worktree
pop-mini/atomicguard            explicit host, main worktree
pop-mini/atomicguard/fix-guards explicit host, named worktree
```

The `dev://` scheme prefix is accepted but optional — `dev atomicguard/fix-guards` and `dev dev://atomicguard/fix-guards` are equivalent. The scheme is stripped before parsing and provides no disambiguation.

**Parsing rules (in order):**

1. Strip optional `dev://` prefix.
2. Split on `/`. One token = project name. Two tokens = ambiguous (see below). Three tokens = `host/project/worktree`.
3. **Two-token ambiguity resolution** — given `a/b`, resolve in this order:
   - If `a` is a member of the set of distinct host strings declared in the config (union of `defaults.host` and all `project.*.host` values) → `host=a, project=b`. Host takes priority even if `a` also matches a project name.
   - Else if `b` matches a worktree of project `a` → `project=a, worktree=b`.
   - Else → error: not found. (`category/project` display name fallback is deferred to a follow-on ADR once project discovery is redesigned for worktree grouping.)
4. Resolved `(host, project, worktree)` tuple is the canonical session identity for all subsequent operations.

### 4. tmux session name mapping

Each host runs an independent tmux server; slug uniqueness is per-server, not global. The canonical tuple is mapped to a tmux-safe slug:

tmux session names cannot contain `/` or `:`. The slug uses `.` as the worktree separator — tmux tolerates `.` in session names.

| Tuple | tmux session name |
|---|---|
| `(local, atomicguard, main)` | `atomicguard` |
| `(local, atomicguard, fix-guards)` | `atomicguard.fix-guards` |
| `(pop-mini, atomicguard, main)` | `atomicguard` *(in pop-mini's tmux server)* |

## Alternatives considered

**Keep INI, add worktree syntax via a new delimiter** — e.g. `atomicguard[fix-guards]=default`. Avoids a format change but produces a bespoke parser that grows complexity with every new field. The existing parser already has edge cases around `@` and `:` splitting; adding `[]` makes it worse. Rejected.

**JSON config** — machine-writable but poor as a human-edited file (no comments, trailing-comma errors). Rejected.

**YAML** — expressive but notorious for footguns (Norway problem, implicit type coercion). The `serde-yaml` crate has had multiple soundness issues. Rejected.

**`dev://` scheme required, not optional** — breaks the existing `dev atomicguard` invocation style without adding disambiguation value. The scheme is cosmetic in this design; the ambiguity resolution rules handle all cases without it. Rejected.

## Consequences

- `config.rs` replaces the hand-written parser with serde + `toml = "0.8"`. The `parse_config_str` function is removed; `parse_config(path: &Path) -> Result<DevConfig>` deserializes directly via `toml::from_str`, then applies `~` expansion to all `path` fields before returning.
- On startup, if `~/.config/dev/config` (INI) exists and `~/.config/dev/config.toml` does not, `dev` exits with a clear error message identifying the format change. No automatic conversion is provided.
- `validate_config` is simplified — TOML parse errors include line/column; structural validation is handled by serde.
- URI parsing is a new module (`uri.rs` or added to `resolve.rs`) returning `(Option<String>, String, Option<String>)` — host, project, worktree.
- The two-token ambiguity rule must be tested explicitly, including the case where a token matches both a host value and a project name (host wins).
- `dev doctor` gains a check: config file parses without error.
- Worktree entry fields (`layout`, `path`, `host`) are defined in the schema now; worktree creation, discovery, and the `category/project` URI fallback are implemented separately.
- `StreamLocalBindUnlink yes` and ControlMaster SSH config are prerequisites for the remote connectivity work that follows from URI host resolution — out of scope here, noted for the next ADR.
