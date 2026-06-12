# Erlang Language Platform (ELP) with etylizer

A fork of [ELP](https://github.com/WhatsApp/erlang-language-platform) that replaces
eqWAlizer with [etylizer](https://github.com/etylizer/etylizer) as the source of type-error
diagnostics. 


## Setup

### 1. Configure your editor

Make the binary executable and point your editor's LSP client at it with the `server` subcommand.

```sh
chmod +x /path/to/elp
```

**VS Code** (ELP extension) `settings.json`:

```json
"elpClient.serverPath": "/path/to/elp",
"elpClient.serverArgs": "server"
```

**coc.nvim** (`coc-settings.json`):

```json
"languageserver": {
  "erlang": {
    "command": "/path/to/elp",
    "args": ["server"],
    "filetypes": ["erlang"],
    "rootPatterns": ["rebar.config", "rebar.lock", ".git", ".jj"]
  }
}
```

`rootPatterns` is recommended so the workspace root is the project root (otherwise it depends on
the editor's working directory).

### 2. Prepare your project

etylizer needs a **rebar3 project that has been built at least once**:

```sh
cd your_project
rebar3 compile
```

**No** diagnostics appear until you compile it once.


## Features

- Diagnostics are computed **on open/save**
- Include directories are taken from ELP's own project model and passed to etylizer, so
  `-include(...)` resolves the same way ELP resolves it.
- Diagnostics show with `source: etylizer` and a code like `etylizer: ty_error`.
