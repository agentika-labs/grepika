interface Config {
  host: string;
  port: number;
  debug: boolean;
}

function createConfig(host: string, port: number): Config {
  return { host, port, debug: false };
}

function validatePort(port: number): boolean {
  return port > 0 && port < 65536;
}

export { createConfig, validatePort };
export type { Config };
