# Using `dev` with devenv

How SSH → `dev <project>` → devenv-activated shell composes into a daily workflow, with no code changes to `dev` itself. Validated end-to-end on pop-mini — see [#14](https://github.com/thompsonson/dev/issues/14).

## The everyday flow

```
ssh <host>
dev <project>          # create or attach tmux session
# pane is ready — devenv environment active, project's scripts on PATH
```

Under the hood:

1. `dev <project>` creates (or attaches to) a tmux session rooted at the project's directory.
2. The pane's zsh sources the direnv hook (installed by [thompsonson/dotfiles#40](https://github.com/thompsonson/dotfiles/pull/40)).
3. `.envrc` runs `use devenv`, devenv evaluates `devenv.nix` and exports the toolchain + scripts.
4. You type `cargo build` / `check` / whatever the project declares — all on PATH.

Zero changes to `dev`'s code. The integration is purely: direnv fires on `cd`, devenv runs in direnv's context.

## Prerequisites (one-time per machine, Linux)

All automated by `chezmoi apply` once the relevant dotfiles PRs land:

| What | From |
|---|---|
| Nix (Determinate installer) | [thompsonson/dotfiles#40](https://github.com/thompsonson/dotfiles/pull/40) |
| `devenv` (`nix profile install`) | #40 |
| `direnv hook zsh` in `dot_zshrc` | #40 |
| `~/.config/direnv/direnvrc` (enables `use devenv`) | [thompsonson/dotfiles#46](https://github.com/thompsonson/dotfiles/pull/46) |

Check the machine is ready: `sysup doctor` should show `nix`, `devenv`, `direnv` with versions.

## One-time per project

Inside a new project directory (Rust, Node, Python, anything devenv supports):

```sh
devenv init
echo 'use devenv' > .envrc     # devenv 2.x does not create this automatically
direnv allow
```

Edit `devenv.nix` to declare the project's languages, packages, scripts, services. See the [devenv reference](https://devenv.sh/reference/options/).

## Worked example: `atomicguard-rs`

The project at [thompsonson/atomicguard-rs](https://github.com/thompsonson/atomicguard-rs) ships a `devenv.nix` with Rust stable + named scripts (`check`, `fix`, `run-example`). Daily flow:

```sh
ssh pop-mini
dev atomicguard-rs             # attach or create

# in the pane:
rustc --version                # /nix/store/... — devenv-pinned toolchain
check                          # fmt --check + clippy + cargo test --workspace
fix                            # cargo fmt --all
run-example                    # cargo run -p ag-cli -- validate ...
```

CI runs the same `check` script via GitHub Actions + devenv, so local == CI. See [`atomicguard-rs/.github/workflows/ci.yml`](https://github.com/thompsonson/atomicguard-rs/blob/main/.github/workflows/ci.yml).

## Session persistence

Sessions survive SSH disconnects — tmux-continuum saves state every 15 min; tmux-resurrect restores on tmux server start. Detach with your tmux prefix + `d`. Next SSH, same `dev <project>` command attaches you back to where you were.

Kill a stale session with `dev kill <project>`, or `dev kill-all` for the whole lot.

## Troubleshooting

**`use_devenv: command not found`** in `.envrc` evaluation
→ `~/.config/direnv/direnvrc` is missing. Either re-apply dotfiles (#46 installs it), or set it up manually:
```sh
devenv direnvrc > ~/.config/direnv/direnvrc
```

**Pane doesn't auto-activate devenv, even though `direnv` is installed**
→ The direnv hook isn't in the pane's zsh. Check: `declare -f _direnv_hook` should print a function. If it doesn't, your zshrc didn't pick up dotfiles #40 — open a new shell or re-apply.

**Stale environment after editing `devenv.nix`**
→ `direnv reload` inside the pane re-evaluates. The devenv library function auto-watches `devenv.nix` / `devenv.lock` so most edits trigger reload on the next prompt.

**First activation hangs for minutes**
→ First-run closure fetch (Rust toolchain, devenv deps) takes 1–3 min on cold nixpkgs. Subsequent activations are sub-second from the store.

**`openssl-sys` build fails**
→ Your project's `devenv.nix` needs `openssl` + `pkg-config` in its `packages` list. Reqwest and other crates with C deps won't link otherwise. CI on ubuntu-latest has `libssl-dev` pre-installed so this failure surfaces only under Nix.

## Related

- [thompsonson/dotfiles `docs/devenv.md`](https://github.com/thompsonson/dotfiles/blob/main/docs/devenv.md) — devenv bootstrap + how `use devenv` works.
- [#10](https://github.com/thompsonson/dev/issues/10) — the integration plan this delivers §1 of.
- [#14](https://github.com/thompsonson/dev/issues/14) — the end-to-end validation test.
