// Platform abstraction types so tests can mock Node's `net` and
// `child_process` modules without pulling in real OS handles. The
// shapes mirror Node's API but only the surface the plugin uses.

export interface NetSocket {
  on(event: "connect", fn: () => void): void;
  on(event: "data", fn: (chunk: Buffer | string) => void): void;
  on(event: "error", fn: (err: Error) => void): void;
  on(event: "close", fn: (hadError: boolean) => void): void;
  write(data: string, cb?: (err?: Error | null) => void): boolean;
  destroy(): void;
}

export interface NetModule {
  createConnection(opts: { path: string }): NetSocket;
}
