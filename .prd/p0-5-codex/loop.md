# loop — P0.5 drive (Codex resolver)

Status: done (2026-10-15; R1 closed in `loop.done.md`). Codex is shipped on `main` (d7b45e3): the
resolver, TOML switches with undo, `harn verify --agent codex`, every surface.

Next start point: P1 (`ssot.md`, out of scope; `01-spec.md` §P1):

- `harn trim --agent codex` (on f8b65da it refuses with exit 3);
- usage evidence in the brief;
- tool schemas from a live run (`CODEX_ROLLOUT_TRACE_ROOT`), read by `verify --capture`.

Open, not scheduled: the installed Codex version is read from npm and Homebrew layouts only, so a
distribution's static binary leaves catalog facts unknown (`01-spec.md` §Source of truth).
