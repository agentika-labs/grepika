interface Replacement {
  id: number;
  value: string;
}

function createReplacement(id: number, value: string): Replacement {
  return { id, value };
}

export { createReplacement };
export type { Replacement };
