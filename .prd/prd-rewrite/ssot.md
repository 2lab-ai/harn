# ssot — PRD authoring drive for `harn` (fixed 2026-10-13)

Status: in-progress

## User statement (verbatim, 2026-10-13, zbrain session, `/goal … /using-dotprd`)

> 이거 프로그램으로 만들어줘 /using-dotprd
>
> 목표:
> - 실행: {프로그램이름} ~/2lab.ai/zbrain
> - 하는일: 해당 위치에서 실행되는 ai agent의 하네스를 모두 리스트업해주고 편집할수 있는 cli 혹은 툴
> - 실행환경: (cachy-z13 리눅스환경과 로컬 macos 환경에서 실행)
> - 아마도 TUI 혹은 gui (필요시), 시스템 툴이니
> - 2lab-ai/{프로그램이름} 으로 repository를 만듬 (llmux repo 참조해서 빌드와 배포 등 같은 오픈소스 방식으로 처리)
>
> 문맥:
> - ~/2lab.ai/zbrain 워킹 폴더에서 claude code를 실행하면
> 1. claude code의 빌트인 프롬프트와 빌트인 스킬들이 로드됨 (로컬에 llmux를 설치했으므로 llmux를 사용하면 이것도 편집할 수 있음)
> 2. ~/.claude 안에 하네스가 로드됨 (프롬프트, 에이전트 스킬, 훅등)
> 3. {워킹폴더}안의 하네스가 로드됨
> 4. 사용자의 지시를 실행함
>
> 의도: claude code와 codex에서 특정 워킹폴더에서 실행됐을때 어떤 하네스들이 로드되는지 어떤 하네스를 삭제해서 최적화할수 있도록 해주는 툴
>
> 해야하는 일:
> - 지금까지 내가 말한 내용을 prd로 정리
> - 이 prd를 기반으로 리서치(비슷한 툴이나 내가 해결하고자 하는 일을 어떻게 하면 편안하게 할수 있는지에 대한 리서치
> - 리서치를 기반으로 가장 좋은 ui와 행동을 할수 있는지 prd를 처음부터 다시 작성

## Fixed scope of this drive

1. `research/00-prd-v1-from-user-statement.md` — the statement above as a PRD, nothing added beyond measured facts.
2. `research/01..03-*.md` — research: Claude Code loading rules, Codex loading rules, prior art + UX patterns.
3. `01-spec.md` (rewritten from scratch on the research) + `02-architecture.md` — reviewed by the three-engine panel (trinity) until unanimous.
4. Repository `2lab-ai/harn` exists on GitHub with this `.prd/` committed. Build/release follow the llmux pattern (`.github/workflows/{ci,preview,release}.yml`, Homebrew tap formulae) — those workflows are specified in 02, not shipped in this drive.

Not in this drive: writing the program.

## Name — `harn` (decided 2026-10-13, veto-able)

The first working name was `loadout`. The prior-art research (`../research/03-prior-art-and-ux.md` §0)
found migsilva89/loadout — a macOS app, 98★, "See and manage what your coding assistants load",
covering Claude Code and Codex — the same job under the same name; crates.io `loadout` is also
taken. Same-space collision is a reason to rename, not to append a suffix.

`harn` = the user's own word for the thing (하네스, harness), clipped to four letters the way
`llmux`/`xfx`/`herdr` are clipped. Checked 2026-10-13: crates.io free, `2lab-ai/harn` free,
Homebrew free, nothing on PATH, no project of that name in the agent-tooling space
(GitHub search returns unrelated "harness" repos). Rejected: `tare` (crate taken, kelviq/tare
295★ is a Claude Code token audit — adjacent), `ballast` (brew + crate taken), `preamble`/`prelude`
(crates taken), `xharness` (dotnet/xharness 188★), `harnix` (tienedev/harnix is a Nix module for
AI agent tooling — adjacent). Binary `harn`, crate `harn`, repo `2lab-ai/harn`. Renaming later
costs one `gh repo rename` plus a tap formula; the user vetoes by saying so.

## Fixed facts the PRD may cite (measured 2026-10-02, zbrain `scratchpad/harness-observe-20261002/inventory.md`)

- Fresh Claude Code 2.1.287 session in `~/2lab.ai/zbrain`, first API request ≈ 231 KB; the user's instruction was 6–61 B of it.
- `tools` array 109 KB / 61 tools; 36 tools are MCP server `blender` (no local config names it — origin unresolved).
- `messages[0]` user blocks: CLAUDE.md ×2 + rules ×2 + MEMORY.md = 45.9 KB; userEmail+gitStatus 3.5 KB; attribution 0.6 KB; instruction last.
- `messages[1]` system text 56 KB: SessionStart hook stdout 3.4 KB, agent roster 16.9 KB, MCP instructions 2 KB, skill list 31 KB (210 skills, 68 with descriptions), Advisor 2 KB.
- Bytes were identical across 5 runs except the instruction and one gitStatus delta.

## User direction (verbatim, 2026-10-13, after seeing migsilva89/loadout)

> loadout을 잠깐 봤는데 그것에 대해 내 생각을 말할께
> - 나랑 같은 지점을 인식하고 그걸 만들었네, 좋은 시도
> - 하지만 gui로 만드는건 실수임 하네스에서 뭘 넣고 뺄지를 내가 결정하는게 말이 안되 어차피 어떤 하네스를 넣을지 뺄지 분석하는것도 ai에게 시켜야 하므로 tui도 아니고 cli로 개발해야함. (단 현재 로드아웃에 대핸 시각화는 gui로 해주는 것은 괜찮은 것같음).
> - 그래서 harn은 특정 프로젝트의 하네스 (시스템 하네스 포함)을 시각화해주고 그 시각화된 내용에 대한 변경 작업은 ai가 할수 있도록 하느데 중점을 줬으면 좋겠음 자세히 안봣는데 저렇게 만들고 실제 설정에 대한 분석과 뭘 켜고 끌지는 쟤도 ai가 하도록 했을것 같긴하다

Consequence fixed here: the operator of harn is an AI agent; the human reads a visualization. No
TUI. A GUI exists only as a read-only view of the current loadout. `01-spec.md` v3.0 carries it.
