# CLAUDE.md — Restate SDK for Unison

**Project state (goals, roadmap, decisions, status): see [PROJECT.md](PROJECT.md).**
Update PROJECT.md whenever decisions are made, work is completed, or the roadmap changes — treat it as a living wiki page, not a snapshot.

## Dev environment

NixOS with per-project Nix shells. Always work inside `nix develop`.

The `.mcp.json` configures the Unison MCP server — it starts automatically when Claude Code is launched from this directory. UCM creates a codebase at `~/.config/unisonlanguage/` on first run. To install libraries, use the MCP `lib-install` tool.

## Unison coding conventions

These are mandatory. Violations cause type errors or subtly wrong behaviour.

**Pattern matching — always use `match/with` or `cases`, never LHS patterns:**
```
-- CORRECT
List.head = cases
  [] -> None
  hd +: _ -> Some hd

-- WRONG (invalid Unison syntax)
List.head [] = None
List.head (hd +: _) = Some hd
```

**Looping — tail recursion with accumulating parameter, build lists forward:**
```
-- CORRECT: O(1) append with :+
List.map f as =
  go acc = cases
    [] -> acc
    x +: xs -> go (acc :+ f x) xs
  go [] as

-- WRONG: not tail-recursive
List.map f = cases
  [] -> []
  x +: xs -> f x +: List.map f xs
```

**No `let`, no `where` — bindings go directly in the block:**
```
foo x =
  y = x + 1   -- CORRECT
  y * 2

-- WRONG
foo x = let y = x + 1 in y * 2
foo x = y * 2 where y = x + 1
```

**Abilities — make higher-order functions ability-polymorphic:**
```
-- CORRECT
List.map : (a ->{g} b) -> [a] ->{g} [b]

-- WRONG: locks out effectful functions
List.map : (a -> b) -> [a] -> [b]
```

**No typeclasses — use explicit dictionary passing:**
```
type Serde a = { encode : a -> Bytes, decode : Bytes -> Either Text a }
-- pass Serde as an argument, don't use implicit resolution
```

**Record field access is via generated functions, not dot notation:**
```
Serde.encode mySerde value   -- CORRECT
mySerde.encode value         -- WRONG
```

**Optional uses `None`/`Some`, not `Nothing`/`Just`.**

**Helper functions:** `go` or `loop` for recursive helpers, `f`/`g` for function args, `acc` for accumulators, `rem` for remainder.

**Tests:** named `foo.tests.examples` (input/output) and `foo.tests.props` (property-based). Use `test>` watch expressions.

## Using the Unison MCP server

Key tools:

- `typecheck-code` — typecheck a `.u` scratch file; use constantly, never show untypechecked code
- `docs` — read docs for any definition (`docs FFI.Readme`, `docs List.map`)
- `view-definitions` — view source of a definition
- `search-definitions-by-name` / `search-by-type` — discover definitions
- `list-project-definitions` / `list-project-libraries` — inspect local codebase
- `lib-install` — install a library from Unison Share
- `update-definitions` — push definitions into the codebase

**Workflow for writing Unison code:**
1. Confirm type signatures before implementing (show user, wait for approval)
2. Write code in a scratch file (`scratch/foo.u`)
3. Typecheck with MCP after each function — never accumulate unchecked code
4. Only show typechecked code to the user
5. Use DEEP WORK mode for multi-function tasks

## Git workflow

- **Linear history** — atomic commits only; each commit must compile, pass tests, and represent one logical change. No merge commits.
- **Commit autonomously** — when a task is complete and format/lint/tests pass, commit without asking. Concise message focused on the why.
