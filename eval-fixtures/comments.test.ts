import { getConfig } from "./comments";

const cfg = getConfig();
console.assert(cfg.timeout === 5000);
console.assert(cfg.retries === 3);
