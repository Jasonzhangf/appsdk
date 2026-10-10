# @jsonstudio/appsdk

Development-only npm launcher candidate for AppSDK. This package is not a
release and must not be published until M5 assembles verified platform
artifacts and the release owner allocates a new source version.

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

The candidate version is derived from the source version by decimal
normalization:

```text
0.1.0014 -> 0.1.14
```

The current package uses `0.1.14-dev.0` because `0.1.14` is already the mapped
release version of `0.1.0014`.
