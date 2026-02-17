import { createAlphaHandler } from "./repetition";

const handler = createAlphaHandler();
console.assert(handler.name === "alpha");
