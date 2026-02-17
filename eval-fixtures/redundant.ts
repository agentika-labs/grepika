// validate the email address format
function validateEmail(email: string): boolean {
  return email.includes("@") && email.includes(".");
}

// check if the port number is valid
function isValidPort(port: number): boolean {
  return port > 0 && port < 65536;
}

// trim whitespace from both ends
function trimInput(input: string): string {
  return input.trim();
}

// Database connection retry interval
const RETRY_INTERVAL = 1000;

// Maximum concurrent connections allowed
const MAX_CONNECTIONS = 10;

// JWT token expiration time
const TOKEN_EXPIRY = 3600;

// Rate limiter window size
const RATE_WINDOW = 60000;

// Cache eviction policy name
const EVICTION_POLICY = "lru";

export {
  validateEmail,
  isValidPort,
  trimInput,
  RETRY_INTERVAL,
  MAX_CONNECTIONS,
  TOKEN_EXPIRY,
  RATE_WINDOW,
  EVICTION_POLICY,
};
