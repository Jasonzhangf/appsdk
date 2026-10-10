# @jsonstudio/appsdk

npm launcher for AppSDK. The platform packages are populated from the verified
release artifacts by the M5 packaging flow.

The package exposes `appsdk` and `project-memory`. Each launcher selects one
exact optional platform package, verifies its version, then replaces itself
with the platform executable while preserving arguments, standard streams,
exit status, and process signals.

Supported dispatch targets:

- macOS ARM64
- Linux x64 with glibc
- Windows x64 with MSVC

Unsupported operating systems, CPUs, Linux ABIs, missing optional packages,
version mismatches, missing binaries, and spawn failures produce explicit
`LauncherError` failures. The launcher never downloads, compiles, or falls back
to a different target.

The package version is derived from the source version by decimal
normalization:

```text
0.1.0015 -> 0.1.15
```

The current package uses `0.1.15`, the mapped release version of `0.1.0015`.
