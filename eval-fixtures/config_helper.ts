interface Options {
  timeout: number;
  retries: number;
}

function getDefaults(): Options {
  return { timeout: 5000, retries: 3 };
}

export { getDefaults };
export type { Options };
