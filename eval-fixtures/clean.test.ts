import { createConfig, validatePort } from "./clean";

const config = createConfig("localhost", 3000);
console.assert(config.host === "localhost");
console.assert(validatePort(3000) === true);
console.assert(validatePort(-1) === false);
