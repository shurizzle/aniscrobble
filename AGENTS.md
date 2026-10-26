# AGENTS.md

Working rules for anyone, human or agent, changing this repository.

## Language

- Code, comments, commit messages, docs: English.
- Chat with the maintainer: Italian.

## Layout

- CLI (`login`, `sync`, `scrobble`): always compiled, blocking, no async runtime.
- TUI (`tui` subcommand): behind the `tui` feature. Its dependencies are
  optional; a CLI-only build must not pull them in.

## Commands

```sh
cargo build                          # CLI + TUI (default)
cargo build --no-default-features    # CLI only
cargo run -- login                   # CLI
cargo run -- tui                     # TUI (needs a real terminal)
cargo test                           # unit tests, TUI included
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Always run `cargo fmt`, `cargo clippy` and `cargo test` before declaring a task
done. Run the build both with and without default features
(`--no-default-features`) when touching `Cargo.toml`, `src/main.rs` or anything
under `src/tui/`. The UI has no automated coverage: when a change touches
rendering or input, state what you checked manually.

## Architecture rules (do not break)

Read `ARCHITECTURE.md` first. These are the load-bearing rules.

CLI:

1. Local progress is written before any network call. `--local-only` only stops
   after that write.
2. An id leaves `pending` only after AniList accepted the update. Never clear it
   optimistically.
3. `scrobble` never lowers progress; an equal or older episode is a no-op.
4. Only database errors are fatal. A failed API call is reported and skipped.

TUI:

5. The main loop must never await network I/O. Requests are spawned.
6. Nothing blocking in the render path. Image decode/encode only inside
   `spawn_blocking`.
7. Every request belongs to a live scope. A scope that is replaced or unmounted
   must have its token cancelled and its tasks aborted.
8. A re-render must not cancel anything. Cancellation is driven by widget
   identity changes, never by frame timing.
9. Stale responses are dropped silently. They must never turn into errors or
   flicker.
10. `select!` in the main loop keeps terminal input first (`biased`).
11. Redraw only when `App::dirty` is set.
12. The TUI only reads the stored login and local progress at startup; it never
    writes account state.
13. Every fetch except search is persisted. Local-first with a per-type TTL,
    except my lists, which are always network-first. The cache is the offline
    fallback in both cases. Search always goes live and has no fallback. Cache
    failures are ignored, never surfaced.

If a change needs to violate one of these, update `ARCHITECTURE.md` in the same
change and say why.

## Feature flag contract

- `tui` is part of `default`; `cargo build --no-default-features` must still
  compile the CLI without TUI dependencies.
- Every TUI dependency is `optional = true` and listed in the `tui` feature with
  `dep:`.
- TUI modules and the `Tui` variant are gated with
  `#[cfg(feature = "tui")]`; code outside `src/tui/` must not name TUI types
  unguarded.
- A bare `aniscrobble` defaults to `tui` only when the feature is enabled.
  With `--no-default-features`, a subcommand stays required.

## Documentation rule (standing)

Every new rule, constraint or decision coming from the maintainer is written into
the documentation **in the same change**, not left in the chat. Map the rule to
its home:

| Kind of rule | Goes to |
| --- | --- |
| Key bindings, screen behaviour, what a key must not do | `README.md` (user facing) and `ARCHITECTURE.md` (semantics) |
| CLI storage/sync semantics, execution model, cancellation, caching, invariants | `ARCHITECTURE.md` |
| Working rules, commands, git policy, this rule itself | `AGENTS.md` |
| Scope, milestone status, what is explicitly out of scope | `docs/roadmap.md` |
| API facts: endpoints, fields, limits | `docs/anilist-api.md` |

Rules to keep honest:

1. If a change makes an existing sentence false, fix or delete that sentence.
2. Do not duplicate the same rule in two docs; link instead.
3. Tick roadmap checkboxes when the acceptance criteria are met, and move
   abandoned items to the "out of scope" list.
4. New docs are created only for a new topic that has no honest home.
5. When a rule is ambiguous, ask before writing it down.

## Contribution style

- Smallest complete change. No speculative abstractions, no unused options.
- Match existing patterns before inventing a new one.
- New non-trivial logic (branching, parsing, concurrency bookkeeping) needs a
  small unit test. Trivial changes do not.
- Known limitations go in a `shortcut: <limit>, <when to upgrade>` comment.

## Git

- Never commit, amend, push, rebase or tag without explicit confirmation from
  the maintainer.
- Branch name is the Jira ticket key when one exists.
- PR base is `develop` for feature/fix branches, `main` only for hotfixes.

## Scope

In:

- CLI: login, local episode tracking, sync, background sync.
- TUI: trending, search, the viewer's current anime/manga lists, detail,
  staff/character pages, cover images, cancellation model.

Out (do not add unless asked):

- TUI login or list mutations.
- Persistence or prefetch inside the TUI.
- Unrequested extra views, options or configuration knobs.
