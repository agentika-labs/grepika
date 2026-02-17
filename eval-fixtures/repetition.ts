interface Handler {
  name: string;
  process(data: unknown): void;
}

function createAlphaHandler(): Handler {
  const name = "alpha";
  const buffer: unknown[] = [];
  const process = (data: unknown) => { buffer.push(data); };
  return { name, process };
}

function createBetaHandler(): Handler {
  const name = "beta";
  const buffer: unknown[] = [];
  const process = (data: unknown) => { buffer.push(data); };
  return { name, process };
}

function createGammaHandler(): Handler {
  const name = "gamma";
  const buffer: unknown[] = [];
  const process = (data: unknown) => { buffer.push(data); };
  return { name, process };
}

function runPipeline(handlers: Handler[], data: unknown[]) {
  for (const item of data) {
    for (const handler of handlers) {
      handler.process(item);
    }
  }
}

export function main() {
  const handlers = [
    createAlphaHandler(),
    createBetaHandler(),
    createGammaHandler(),
  ];
  runPipeline(handlers, [1, 2, 3]);
}

export { createAlphaHandler, createBetaHandler, createGammaHandler };
