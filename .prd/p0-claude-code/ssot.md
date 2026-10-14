# ssot — P0 drive: Claude Code chain (fixed 2026-10-13)

Status: done (2026-10-13; receipts in `loop.done.md`)

## User statement (verbatim, 2026-10-13, `/goal`)

> prd대로 작업 진행

## Fixed target

`01-spec.md` v2.9 §P0 and acceptance rows **A1–A6, A9–A11**, on the author's macOS and on
`cachy-z13` (Linux x86_64, cargo 1.96.1, `~/.local/bin/claude` present, no `codex`). P0.5 (Codex,
A7–A8) and P1 (TUI) are out of this drive. Interfaces between work units are fixed in
[`../03-p0-interfaces.md`](../03-p0-interfaces.md); a WU that needs to change an interface says so
in its report instead of editing another WU's files.

## Gate

`just check` = `cargo fmt --check` · `cargo clippy --all-targets --locked -- -D warnings` ·
`cargo test --locked`. Differential tests run only when `claude` is on PATH (skipped in CI).

## Receipts (what closes a row)

Each A-row: the exact command, its captured output (file under the scratchpad, path recorded in
`loop.done.md`), on both machines where the row says so.

## User direction (verbatim, 2026-10-13, after seeing migsilva89/loadout)

> loadout을 잠깐 봤는데 그것에 대해 내 생각을 말할께
> - 나랑 같은 지점을 인식하고 그걸 만들었네, 좋은 시도
> - 하지만 gui로 만드는건 실수임 하네스에서 뭘 넣고 뺄지를 내가 결정하는게 말이 안되 어차피 어떤 하네스를 넣을지 뺄지 분석하는것도 ai에게 시켜야 하므로 tui도 아니고 cli로 개발해야함. (단 현재 로드아웃에 대핸 시각화는 gui로 해주는 것은 괜찮은 것같음).
> - 그래서 harn은 특정 프로젝트의 하네스 (시스템 하네스 포함)을 시각화해주고 그 시각화된 내용에 대한 변경 작업은 ai가 할수 있도록 하느데 중점을 줬으면 좋겠음 자세히 안봣는데 저렇게 만들고 실제 설정에 대한 분석과 뭘 켜고 끌지는 쟤도 ai가 하도록 했을것 같긴하다

Consequence fixed here: the operator of harn is an AI agent; the human reads a visualization. No
TUI. A GUI exists only as a read-only view of the current loadout. `01-spec.md` v3.0 carries it.
