# AppSDK npm packages

This directory contains the npm launcher and platform package metadata for
`@jsonstudio/appsdk`. The platform packages are populated from the verified
release artifacts by the M5 packaging flow; this repository does not store
those binary and Skill payloads.

The main package has two launchers:

- `appsdk` -> `@jsonstudio/appsdk`
- `project-memory` -> `@jsonstudio/appsdk`

The launcher dispatches to one exact optional platform package:

| Target | npm package | Runtime restriction |
|---|---|---|
| macOS ARM64 | `@jsonstudio/appsdk-darwin-arm64` | `os=darwin`, `cpu=arm64` |
| Linux x64 GNU | `@jsonstudio/appsdk-linux-x64-gnu` | `os=linux`, `cpu=x64`, `libc=glibc` |
| Windows x64 MSVC | `@jsonstudio/appsdk-win32-x64-msvc` | `os=win32`, `cpu=x64` |

Each assembled platform package contains:

```text
bin/appsdk
bin/project-memory
skills/appsdk-project-governance/**
skills/appsdk-migration/**
skills/project-memory/**
```

Windows uses `bin/appsdk.exe` and `bin/project-memory.exe`.

M5 assembles the verified platform artifacts into these paths before a package
is packed or installed. The tracked manifests contain no binary, Skill, hash,
or success shim.

## Version mapping

The source version is normalized by decimal components:

```text
0.1.0015 -> 0.1.15
```

The current source version is `0.1.0015`, so the formal npm version is
`0.1.15`. A release owner must bind one commit and one verified artifact set
before publication.

No npm package in this directory runs a `postinstall` hook, downloads runtime
artifacts, compiles Rust, or writes a canonical global installation.
