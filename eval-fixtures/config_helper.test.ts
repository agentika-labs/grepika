import { getDefaults } from "./config_helper";

const opts = getDefaults();
console.assert(opts.timeout === 5000);
console.assert(opts.retries === 3);
