export class LauncherError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "LauncherError";
    this.code = code;
  }
}
