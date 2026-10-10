# @jsonstudio/appsdk-linux-x64-gnu

Linux x64 glibc runtime package metadata for AppSDK.

The M5 packaging flow assembles the verified `appsdk` and `project-memory`
GNU/glibc binaries plus the three managed Skills into the paths declared by
`files` before this package is packed or installed. The tracked metadata does
not contain binary, Skill, or placeholder payloads. musl is not supported by
this package.
