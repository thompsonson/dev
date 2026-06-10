# Glossary

Shared terms for `dev` domain concepts. These names are intentionally small and concrete; update this file when ADRs introduce or retire concepts.

## Config

**Raw config**

The TOML-shaped data loaded from `~/.config/dev/config.toml`. Raw config mirrors the file format and exists only at the IO boundary.

**RawDevConfig**

The top-level raw TOML config struct. Contains `RawDefaults` and project entries exactly as authored.

**RawDefaults**

The raw `[defaults]` table. Fields are optional because omitted values may be filled by domain defaults.

**RawProjectEntry**

The raw `[project.<name>]` table. Fields are optional because project config can inherit from defaults.

**RawWorktreeEntry**

The raw `[project.<name>.worktree.<name>]` table. Fields are optional because worktree config can inherit from its project and defaults.

**Domain config**

The application-facing config model. It resolves raw TOML into stable domain concepts and is the config surface used by `api.rs`, daemon handlers, URI resolution, and worktree logic.

**DevConfig**

The domain config. It owns inheritance, defaulting, path expansion, and config-level validation. Code outside `config.rs` should depend on `DevConfig`, not raw TOML structs.

**ResolvedSessionConfig**

The effective config for one session identity. It contains resolved layout, host, optional path, and other values after applying inheritance in the order defaults -> project -> worktree.

## Session Identity

**Project**

A named development unit managed by `dev`. Usually maps to a git repository under `~/Projects`, or to a configured custom path.

**Worktree**

A named git linked worktree for a project. The main worktree is represented by `None`; named worktrees are represented explicitly.

**Host**

The machine where a session runs. Hosts are read from config and used for routing. Each host has its own tmux server namespace.

**Session URI**

The user-facing address for a session: `[host/]project[/worktree]`, with optional `dev://` prefix.

**Resolved session identity**

The canonical `(host, project, worktree)` tuple produced after parsing and resolving a user request. This identity drives routing and tmux session naming.

**Tmux session name**

The tmux-safe slug derived from resolved session identity. Main worktree sessions use the project name; named worktrees use `project.worktree`.
