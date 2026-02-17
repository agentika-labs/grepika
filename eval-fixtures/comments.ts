// TCP connection timeout in milliseconds
const TCP_TIMEOUT = 5000;

// Maximum retry attempts before giving up
const MAX_RETRIES = 3;

// Size of the internal buffer pool
const BUFFER_SIZE = 4096;

// DNS resolution cache TTL
const DNS_TTL = 300;

// Socket backlog limit
const BACKLOG = 128;

// Keepalive interval between probes
const KEEPALIVE_MS = 60000;

// Port range start
const PORT_MIN = 1024;

// Port range end
const PORT_MAX = 65535;

// Default bind address
const BIND_ADDR = "0.0.0.0";

// Protocol version identifier
const PROTO_VERSION = 2;

export function getConfig() {
  return {
    timeout: TCP_TIMEOUT,
    retries: MAX_RETRIES,
    bufferSize: BUFFER_SIZE,
    dnsTtl: DNS_TTL,
    backlog: BACKLOG,
    keepalive: KEEPALIVE_MS,
    portMin: PORT_MIN,
    portMax: PORT_MAX,
    bindAddr: BIND_ADDR,
    protoVersion: PROTO_VERSION,
  };
}
