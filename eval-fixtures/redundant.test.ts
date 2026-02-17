import { validateEmail, isValidPort } from "./redundant";

console.assert(validateEmail("a@b.c") === true);
console.assert(isValidPort(8080) === true);
