import { createReplacement } from "./replacement";

const r = createReplacement(1, "test");
console.assert(r.id === 1);
