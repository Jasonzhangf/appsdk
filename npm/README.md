# AppSDK npm candidate

This directory contains the development-only npm wrapper candidate for
`@jsonstudio/appsdk`. It does not contain verified AppSDK binaries or Skills
and is not a distributable release candidate.

The main package has two launchers:

- `appsdk` -> `@jsonstudio/appsdk`
- `project-memory` -> `@jsonstudio/appsdk`

The launcher dispatches to one exact optional platform package:

| Target | npm package | Runtime restriction |
|---|---|---|
| macOS ARM64 | `@jsonstudio/appsdk-darwin-arm64` | `os=darwin`, `cpu=arm64` |
| Linux x64 GNU | `@jsonstudio/appsdk-linux-x64-gnu` | `os=linux`, `cpu=x64`, `libc=glibc` |
| Windows x64 MSVC | `@jsonstudio/appsdk-win32-x64-msvc` | `os=win32`, `cpu=x64` |

Each platform package must eventually contain:

```text
bin/appsdk
bin/project-memory
skills/appsdk-project-governance/**
skills/appsdk-migration/**
skills/project-memory/**
```

Windows uses `bin/appsdk.exe` and `bin/project-memory.exe`.

M5 must assemble the verified M1/M2 artifacts into these paths before a package
can be packed, installed, or published. The current manifests intentionally
contain no binary, Skill, hash, or success shim.

## Version mapping

The source version is normalized by decimal components:

```text
0.1.0014 -> 0.1.14
```

The current source version is already represented by an existing release
mapping. These files therefore use the development-only version
`0.1.14-dev.0` and are marked private. A release owner must allocate a new
source version, bind one commit and one verified artifact set, and remove the
candidate marker before publication.

No npm package in this directory runs a `postinstall` hook, downloads runtime
artifacts, compiles Rust, or writes a canonical global installation.
