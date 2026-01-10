//! MaybeSecureStream - wrapper for optionally TLS-encrypted streams

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::server::TlsStream;

/// A stream that may or may not be TLS-encrypted
///
/// This allows the server to handle both plain and TLS connections
/// with the same code path.
pub enum MaybeSecureStream {
    /// Plain TCP stream (no encryption)
    Plain(TcpStream),
    /// TLS-encrypted stream
    Tls(TlsStream<TcpStream>),
}

impl MaybeSecureStream {
    /// Create a plain (unencrypted) stream
    pub fn plain(stream: TcpStream) -> Self {
        MaybeSecureStream::Plain(stream)
    }

    /// Create a TLS-encrypted stream
    pub fn tls(stream: TlsStream<TcpStream>) -> Self {
        MaybeSecureStream::Tls(stream)
    }

    /// Check if this is a TLS connection
    pub fn is_tls(&self) -> bool {
        matches!(self, MaybeSecureStream::Tls(_))
    }

    /// Get peer address (works for both plain and TLS)
    pub fn peer_addr(&self) -> io::Result<std::net::SocketAddr> {
        match self {
            MaybeSecureStream::Plain(s) => s.peer_addr(),
            MaybeSecureStream::Tls(s) => s.get_ref().0.peer_addr(),
        }
    }

    /// Set TCP_NODELAY
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        match self {
            MaybeSecureStream::Plain(s) => s.set_nodelay(nodelay),
            MaybeSecureStream::Tls(s) => s.get_ref().0.set_nodelay(nodelay),
        }
    }

    /// Get peer certificates (for client certificate authentication)
    ///
    /// Returns the client's certificate chain if this is a TLS connection
    /// and the client provided certificates during handshake.
    pub fn peer_certificates(&self) -> Option<Vec<CertificateDer<'static>>> {
        match self {
            MaybeSecureStream::Plain(_) => None,
            MaybeSecureStream::Tls(s) => {
                let (_, conn) = s.get_ref();
                conn.peer_certificates().map(|certs| certs.to_vec())
            }
        }
    }

    /// Extract the Common Name (CN) from the client certificate
    ///
    /// Returns the CN from the subject of the first certificate in the chain.
    /// This is typically used for client certificate authentication to identify the user.
    pub fn peer_certificate_cn(&self) -> Option<String> {
        let certs = self.peer_certificates()?;
        let cert_der = certs.first()?;

        // Parse the X.509 certificate
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref()).ok()?;

        // Extract the Common Name from the subject
        for rdn in cert.subject().iter() {
            for attr in rdn.iter() {
                if attr.attr_type() == &x509_parser::oid_registry::OID_X509_COMMON_NAME {
                    return attr.attr_value().as_str().ok().map(|s| s.to_string());
                }
            }
        }

        None
    }
}

impl AsyncRead for MaybeSecureStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeSecureStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeSecureStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeSecureStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeSecureStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeSecureStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeSecureStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeSecureStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeSecureStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeSecureStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeSecureStream::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            MaybeSecureStream::Tls(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            MaybeSecureStream::Plain(s) => s.is_write_vectored(),
            MaybeSecureStream::Tls(s) => s.is_write_vectored(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_is_tls() {
        // We can't easily create TcpStream in tests, but we can verify the enum structure
        // Real integration tests would use actual connections
    }
}
