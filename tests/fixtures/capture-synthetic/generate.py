#!/usr/bin/env python3
"""Writes the synthetic Claude Code capture used by tests/verify.rs and prints its region table.

Every string here is invented; only Claude Code's own section headers are reproduced. The body is
compact JSON (as Claude Code sends it), so `serde_json` re-serialises each value to the same bytes.
The region arithmetic below is an independent re-implementation of the rule documented in
src/oracle/claude.rs (a section starts at the newline before its header line and runs to the
newline before the next header), so the numbers it prints are the test's oracle.

Run from anywhere: python3 -I tests/fixtures/capture-synthetic/generate.py
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECT = "/synthetic/project"
HOME = "/synthetic/home"
MEMORY = HOME + "/.claude/projects/-synthetic-project/memory/MEMORY.md"
SESSION = "00000000-0000-4000-8000-000000000001"


def compact(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def nbytes(text):
    return len(text.encode("utf-8"))


claude_md = '# Demo project\n\nRules — keep it "simple".\n'
memory_md = "- [note one](n1.md) — first\n- [note two](n2.md) — second\n"
block0 = (
    "<system-reminder>\n"
    "Synthetic preamble for the instruction files.\n"
    "\n"
    "Contents of " + PROJECT + "/CLAUDE.md (project instructions, checked into the codebase):\n"
    "\n" + claude_md + "\n"
    "Contents of " + MEMORY + " (user's auto-memory, persists across conversations):\n"
    "\n" + memory_md + "</system-reminder>"
)
block1 = (
    "<system-reminder>\n"
    "Synthetic context preamble:\n"
    "# userEmail\n"
    "The user's email address is dev@example.invalid.\n"
    "# gitStatus\n"
    "Synthetic git status.\n"
    "\n"
    "Current branch: main\n"
    "\n"
    "Status:\n"
    "M src/lib.rs\n"
    "</system-reminder>\n"
)
hook = "SessionStart hook additional context: synthetic hook output — line 1\nline 2\n"
environment = (
    "\n# Environment\n"
    "\n"
    " - Primary working directory: " + PROJECT + "\n"
    " - Is a git repository: true\n"
)
roster_entries = [
    "- general-purpose: General agent. (Tools: *)\n",
    "- reviewer: Reviews diffs.\n  Second line of the reviewer description.\n",
    "- demo:helper: Plugin agent. (Tools: Read)\n",
]
roster = (
    "\nAvailable agent types for the Agent tool:\n"
    + "".join(roster_entries)
    + "\nWhen you launch several agents, send them together.\n"
)
mcp_server_section = "\n## plugin:demo:demo\nUse search before fetch.\n# Notes\nAn inner header that is not a section.\n"
mcp = (
    "\n# MCP Server Instructions\n"
    "\n"
    "The following MCP servers have provided instructions for how to use their tools and resources:\n"
    + mcp_server_section
)
skill_entries = [
    "- alpha: Does alpha things — quickly.\n",
    "- beta\n",
    "- demo:gamma: Plugin skill gamma.\n",
    "- delta (delta-alias)\n",
]
skills = "\nThe following skills are available for use with the Skill tool:\n\n" + "".join(skill_entries)
date = "\nToday's date is 2026-01-02.\n"
advisor = "\n# Advisor Tool\n\nYou have an advisor."
system_text = hook + environment + roster + mcp + skills + date + advisor

tools = [
    {
        "name": "Read",
        "description": "Reads a file — any file.",
        "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]},
    },
    {
        "name": "mcp__plugin_demo_demo__search",
        "description": "Search the demo index.",
        "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}},
    },
    {
        "name": "mcp__plugin_demo_demo__fetch",
        "description": "Fetch one demo record.",
        "input_schema": {"type": "object", "properties": {"id": {"type": "integer"}}},
    },
    {"type": "advisor_20260301", "name": "advisor", "model": "claude-test-1"},
]
system = [
    {"type": "text", "text": "x-anthropic-billing-header: cc_version=9.8.7.123; cc_entrypoint=sdk-cli;"},
    {"type": "text", "text": "You are a synthetic agent.", "cache_control": {"type": "ephemeral"}},
    {
        "type": "text",
        "text": '\nSynthetic harness rules — use "quotes"\tand tabs.\n# Section\nMore rules.',
        "cache_control": {"type": "ephemeral"},
    },
]
tool_addition = {"type": "tool_addition", "tool": {"type": "advisor_20260301", "name": "advisor"}, "cache_control": {"type": "ephemeral"}}
body = {
    "model": "claude-test-1",
    "messages": [
        {
            "role": "user",
            "content": [
                {"type": "text", "text": block0},
                {"type": "text", "text": block1},
                {"type": "text", "text": "hi"},
            ],
        },
        {"role": "system", "content": [{"type": "text", "text": system_text}, tool_addition]},
    ],
    "system": system,
    "tools": tools,
    "betas": ["beta-one", "beta-two"],
    "metadata": {"user_id": "synthetic"},
    "max_tokens": 1024,
    "thinking": {"type": "adaptive"},
    "stream": True,
}
raw = compact(body)

# ---- region oracle ---------------------------------------------------------------------------
regions = []
regions.append(("tools · builtin (2 tools)", nbytes(compact(tools[0])) + nbytes(compact(tools[3]))))
regions.append(("tools · mcp plugin:demo:demo (2 tools)", nbytes(compact(tools[1])) + nbytes(compact(tools[2]))))
regions.append(("system · billing header", nbytes(system[0]["text"])))
regions.append(("system · identity", nbytes(system[1]["text"])))
regions.append(("system · harness rules", nbytes(system[2]["text"])))
# block 0: wrapper = preamble (before the first "\nContents of") + the closing tag
i_claude = block0.index("\nContents of " + PROJECT)
i_memory = block0.index("\nContents of " + MEMORY)
i_close = block0.rindex("</system-reminder>")
regions.append(("messages[0] block 0 · reminder wrapper", nbytes(block0[:i_claude]) + nbytes(block0[i_close:])))
regions.append(("messages[0] block 0 · " + PROJECT + "/CLAUDE.md", nbytes(block0[i_claude:i_memory])))
regions.append(("messages[0] block 0 · auto-memory " + MEMORY, nbytes(block0[i_memory:i_close])))
i_email = block1.index("\n# userEmail")
i_git = block1.index("\n# gitStatus")
j_close = block1.rindex("</system-reminder>")
regions.append(("messages[0] block 1 · reminder wrapper", nbytes(block1[:i_email]) + nbytes(block1[j_close:])))
regions.append(("messages[0] block 1 · userEmail", nbytes(block1[i_email:i_git])))
regions.append(("messages[0] block 1 · gitStatus", nbytes(block1[i_git:j_close])))
regions.append(("messages[0] block 2 · instruction", 2))
regions.append(("messages[1] system text · SessionStart hook stdout", nbytes(hook)))
regions.append(("messages[1] system text · Environment", nbytes(environment)))
regions.append(("messages[1] system text · agent roster (3)", nbytes(roster)))
regions.append(("messages[1] system text · MCP instructions", nbytes(mcp)))
regions.append(("messages[1] system text · skill list (4 entries, 2 with descriptions)", nbytes(skills)))
regions.append(("messages[1] system text · date", nbytes(date)))
regions.append(("messages[1] system text · Advisor", nbytes(advisor)))
regions.append(("messages[1] tool_addition", nbytes(compact(tool_addition))))
params = {k: v for k, v in body.items() if k not in ("messages", "system", "tools")}
regions.append(("request parameters", nbytes(compact(params))))
attributed = sum(b for _, b in regions)
regions.append(("framing", nbytes(raw) - attributed))

# ---- items whose bytes the test pins ---------------------------------------------------------
items = {
    "instruction CLAUDE.md": nbytes(block0[i_claude:i_memory]),
    "CLAUDE.md file bytes": nbytes(claude_md),
    "memory": nbytes(block0[i_memory:i_close]),
    "roster entries": [nbytes(e) for e in roster_entries],
    "skill entries": [nbytes(e) for e in skill_entries],
    "mcp instructions plugin:demo:demo": nbytes(mcp_server_section),
    "mcp tools": [nbytes(compact(t)) for t in tools[1:3]],
    "builtin tools": [nbytes(compact(tools[0])), nbytes(compact(tools[3]))],
}

index = {
    "timestamp": "2026-01-02T03:04:05.678Z",
    "session_id": SESSION,
    "query_source": "sdk",
    "model": "claude-test-1",
    "request_id": "req_synthetic",
    "message_id": "msg_synthetic",
    "message_uuid": "00000000-0000-4000-8000-0000000000bb",
    "request_file": "synthetic.request.json",
    "response_file": "req_synthetic.response.json",
}
hook_started = {"type": "system", "subtype": "hook_started", "hook_id": "h1", "hook_name": "SessionStart:startup", "hook_event": "SessionStart", "session_id": SESSION}
hook_response = dict(hook_started, subtype="hook_response", stdout="synthetic", stderr="", exit_code=0, outcome="success")
init = {
    "type": "system",
    "subtype": "init",
    "cwd": PROJECT,
    "session_id": SESSION,
    "tools": ["Read", "mcp__plugin_demo_demo__search", "mcp__plugin_demo_demo__fetch"],
    "mcp_servers": [
        {"name": "plugin:demo:demo", "status": "connected", "source": "plugin"},
        {"name": "plugin:off:off", "status": "disabled", "source": "plugin"},
    ],
    "model": "claude-test-1",
    "permissionMode": "default",
    "slash_commands": ["demo:gamma"],
    "claude_code_version": "9.8.7",
    "agents": ["general-purpose", "reviewer", "demo:helper"],
    "skills": ["alpha", "beta", "demo:gamma", "delta"],
    "plugins": [{"name": "demo", "path": HOME + "/.claude/plugins/cache/market/demo/1.0.0", "source": "demo@market", "version": "1.0.0"}],
    "memory_paths": {"auto": HOME + "/.claude/projects/-synthetic-project/memory/"},
}
result = {"type": "result", "subtype": "error_max_turns", "session_id": SESSION}

if "--check" not in sys.argv:
    with open(os.path.join(HERE, "synthetic.request.json"), "w", encoding="utf-8", newline="") as f:
        f.write(raw)
    with open(os.path.join(HERE, "index.jsonl"), "w", encoding="utf-8", newline="") as f:
        f.write(compact(index) + "\n")
    with open(os.path.join(HERE, "stream.jsonl"), "w", encoding="utf-8", newline="") as f:
        for event in (hook_started, hook_response, init, result):
            f.write(compact(event) + "\n")

print("body", nbytes(raw))
for name, size in regions:
    print(f"{size}\t{name}")
print("sum", sum(b for _, b in regions))
for key, value in items.items():
    print(key, value)
