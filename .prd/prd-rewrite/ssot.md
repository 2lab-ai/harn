# ssot — PRD authoring drive for `harn` (fixed 2026-10-13)

Status: done (2026-10-13; receipts in `loop.done.md`)

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

## 유저 원문 3 (2026-10-13 17:1x, 설치 직후 `harn` 실행 후)

> 니가 만든걸 어케 쓰는데 병신아 처음에 설치하고 harn을 치면 어떻게 쓰는지 설명을 해줘야지 시발 그냥 좇나 텍스트만 10만줄 나오는데 뭘 어쩌라는거야?

> 아니 씨발 니가 말한 1번 2번을 사람이 하려면 시팔 니가 좇같이 만든거잖아 tui로 만들었어야지 병신아

읽기: 두 번째 문장은 TUI 재요구가 아니라 귀류다 — "사람이 행 이름을 쳐서 why/off를 하라고 할 거면 TUI를 만들었어야지" = 사람에게 그 일을 시키는 것 자체가 설계 위반(원문 2: 넣고 뺄지는 AI). 고정 스코프: 사람이 치는 명령은 `harn`(보기)·`harn trim`(AI가 수행)·`harn report --html`뿐이고, 행 이름은 사람이 치지 않는다. TUI는 여전히 비목표.

## 유저 원문 4–6 (2026-10-13 17:2x–17:3x, 같은 대화)

> 아니 시팔 디폴트는 ai에게 물어서 ai 스킬을 추가해서 ai가 harn을 사용해서 고치는건 ok 근데 그래도 사람이 메뉴얼로 픽스하고 실제로 뭐가 있고 뭘 없얠수 있는지를 확인하려면 최소한 tui나 gui가 있어야하잖아 병신이야?

> 니가 씨팔 그냥 tui로 해야할껄 좇같이 수백라인 출력하고 잇으니까 병ㅅ힌새끼야 그건 그냥 tui를 안만든 병신인거지

> 수백라인 출력할 내용이 없으면 tui가 필요 없는거고 병신아 내가 무슨말하는건지 알아?

읽기(고정): ① 결정의 디폴트는 AI(`harn trim` / `/harn-trim`). ② 사람이 "지금 뭐가 실려 있고 뭘 뺄 수 있는지" 직접 보고 손으로 고치는 화면이 반드시 있다 — 그 화면이 바로 터미널에서 `harn`을 쳤을 때 뜨는 TUI다. ③ 내용이 수백 줄이라는 사실 자체가 TUI의 근거다: 줄여서 요약하지 말고 전부를 접고·펴고·찾아 들어가게 한다. 원문 2의 "TUI도 아니고 CLI"는 "사람이 GUI에서 결정하게 만드는 것"에 대한 거부였지 사람용 화면 삭제 지시가 아니었다(v3.0에서 과잉 해석). GUI(HTML 리포트)는 공유용 읽기 전용으로 유지.

## 유저 원문 7 (2026-10-13 18:2x, R4 TUI 프레임을 본 뒤)

> 병신아 니가 tui 몇개 텍스트 스크린샷으로 저장하고 그걸로 씨발 리뷰해봐 좇같은새끼야 이게 씨발 tui임? 씨발 가독성 씨발 0%짜리 그냥 기존껄 tui라고 말할수만 있도록 ui만 붙였네 이 병신 좇같은새끼야

읽기: 출하 판정의 기준은 "사람이 그 화면을 읽을 수 있는가"이고, 그 판정은 캡처를 사람(디스패처) 눈으로 읽어야만 내릴 수 있다. 폭·키·바이트 복원 검사는 그 대체물이 아니다. R4 TUI는 CLI 렌더러 문자열을 상자에 넣은 것 → `04-tui-design.md`로 화면을 다시 정의하고 A16으로 인수한다.
