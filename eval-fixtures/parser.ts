// TCP receive buffer size
const RECV_BUF = 8192;

// UDP datagram max size
const UDP_MAX = 65507;

// SCTP association timeout
const SCTP_TIMEOUT = 30000;

// QUIC handshake deadline
const QUIC_DEADLINE = 5000;

// DTLS record size limit
const DTLS_LIMIT = 16384;

// WebSocket ping interval
const WS_PING = 25000;

// gRPC keepalive timeout
const GRPC_KA = 20000;

// HTTP2 max frame size
const H2_FRAME = 16384;

// TLS session ticket lifetime
const TLS_TICKET = 7200;

// ALPN protocol identifier
const ALPN_ID = "h2";

export function createParser() {
  return {
    recvBuf: RECV_BUF,
    udpMax: UDP_MAX,
    sctpTimeout: SCTP_TIMEOUT,
    quicDeadline: QUIC_DEADLINE,
    dtlsLimit: DTLS_LIMIT,
    wsPing: WS_PING,
    grpcKa: GRPC_KA,
    h2Frame: H2_FRAME,
    tlsTicket: TLS_TICKET,
    alpnId: ALPN_ID,
  };
}
