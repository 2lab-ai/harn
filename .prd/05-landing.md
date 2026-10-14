# harn — Landing page and site

Status: **shipped** — v1.0, 2026-10-14. The site is live at https://harn.2lab.ai, built from
`docs/landing` on `main`. This file describes the shipped contract, not a future target.

## User statements (verbatim)

2026-10-13, `/goal`:

> 메인 페이지랑 그리고 이걸 소개하는 페이지를 만들어줘 도메인은 나중에 추가해줄테니까 1. 문서 정리(리드미) 2. 2lab-ai/harn/docs/landing 에 harn.dev(가제 아마 이 도메인은 못살꺼야) 랜딩 페이지 만들어줘. loadout.{}.dev 그 페이지랑 herdr.dev 참조해서 랜딩 페이지 ui-ux 스킬도 사용하고 깔끔한 블랙&화이트로 만들어줘

Added during the same goal:

> 3. 그리고 이걸 cloudflare pages로 일단 올려줘. 자동으로 docs/landing 페이지가 업데이트될떄마다 ci로 자동 배포 처리

2026-10-14:

> 이거 harn.2lab.ai 로 도메인 연결해

## What the site is

One static page plus a 404, in `docs/landing/` (no build step, no framework, about 680 KB):

- `index.html` — hero with the install command, a numbers strip, a terminal plate with seven tabs
  of real harn frames (verbatim captures, not mock-ups), how it works in five steps, principles,
  FAQ, install call to action, footer.
- `style.css` — black and white only: tokens on `:root`, a dark palette for
  `prefers-color-scheme: dark` and for the manual toggle. Type: Inter for text, JetBrains Mono for
  terminal frames; both self-hosted under `fonts/` with their OFL licenses.
- `site.js` — tabs, copy buttons, theme toggle. Loaded from `<head>` as a file; no inline script.
- `_headers` — Cloudflare Pages response headers: a CSP of `'self'` only, `nosniff`,
  `X-Frame-Options: DENY`, a referrer policy; five-minute cache for html/css/js, immutable fonts.
- `og.png` (1200×630), `favicon.svg`, `404.html`.

Design references: the ui-ux skill's default `openai` reference (pure black and white, no accent
colour), herdr.dev and the loadout landing page for layout. Nothing loads from another host.

## Deployment

| part | value |
|---|---|
| host | Cloudflare Pages, project `harn-landing` |
| source | the Cloudflare Workers and Pages GitHub app on `2lab-ai`, limited to `harn` |
| build | no build command; output directory `docs/landing`; production branch `main` |
| trigger | a push to `main` that changes `docs/landing/*` (Pages build watch path); preview builds off |
| domain | `harn.2lab.ai` — Pages custom domain; DNS = `CNAME harn → harn-landing.pages.dev` in the `2lab.ai` zone (hosted at Vercel) |
| fallback address | https://harn-landing.pages.dev |

In `index.html`, `<link rel="canonical">` and `og:url` are the page address `https://harn.2lab.ai/`;
`og:image` is the preview image on the same host, `https://harn.2lab.ai/og.png`.

## Check (CI)

`.github/workflows/landing.yml` runs on every pull request and push that touches `docs/landing/**`
or the workflow. It fails when:

- a referenced file (`src`, `href` of a `<link>`, CSS `url()`, `og:image`) does not exist;
- anything loads from another host;
- an inline `<script>`, `<style>`, `style=` or `on*=` attribute is present (the CSP blocks them);
- an in-page `#anchor` has no target;
- canonical or `og:url` is not `https://harn.2lab.ai/`;
- the directory is 2 MB or larger.

The workflow only checks. Cloudflare deploys.

## Changing the site

Edit `docs/landing/`, open a pull request, let the check pass, merge. Cloudflare builds the merge
commit and serves it about 30 seconds later. A frame on the page that comes from harn's screen is
re-captured from the real binary when the screen changes; it is never redrawn by hand.

## Acceptance (observed 2026-10-14)

| id | run | observed |
|---|---|---|
| L1 | open https://harn.2lab.ai | 200, the harn page; valid certificate; CSP and frame headers present |
| L2 | merge a pull request that touches `docs/landing` (#6 `febde1f`, #7 `0de8dcd`) | a `github:push` Pages build for that exact commit succeeded within 30 s, with no manual step |
| L3 | the landing check on a copy with a wrong canonical and a missing `og:image` | exit 1 with both errors named |
| L4 | the page at 1440, 1024, 768 and 390 px, light and dark | no horizontal scroll; frames legible; read from screenshots before the first publish |
