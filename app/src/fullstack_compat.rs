//! Compatibility module for types that are only available when the `fullstack`
//! feature is enabled.
//!
//! When `fullstack` is active we re-export directly from `dioxus::fullstack`.
//! When it is not (e.g. `demo` mode without a backend), we provide a minimal
//! local replacement so the rest of the crate still compiles.

// ---------------------------------------------------------------------------
// Feature-gated type selection
// ---------------------------------------------------------------------------

#[cfg(not(feature = "fullstack"))]
pub(crate) mod inner {
    use bytes::Bytes;
    use futures::{Stream, StreamExt};
    use std::pin::Pin;

    type DynStream = Pin<Box<dyn Stream<Item = Result<Bytes, StreamingError>> + Send>>;

    /// A streaming payload of bytes — local replacement for
    /// `dioxus_fullstack::payloads::stream::ByteStream`.
    pub struct ByteStream {
        inner: DynStream,
    }

    /// Errors that can occur while streaming.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub enum StreamingError {
        /// The streaming request was interrupted and could not be completed.
        Interrupted,
        /// The stream failed to decode a chunk.
        Decoding,
        /// The streaming request failed.
        Failed,
    }

    impl std::fmt::Display for StreamingError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Interrupted => write!(f, "The streaming request was interrupted"),
                Self::Decoding => write!(f, "The stream failed to decode a chunk"),
                Self::Failed => write!(f, "The streaming request failed"),
            }
        }
    }

    impl std::error::Error for StreamingError {}

    impl ByteStream {
        /// Create a new `ByteStream` from any [`Stream`] of [`Bytes`].
        pub fn new(stream: impl Stream<Item = Bytes> + Send + 'static) -> Self {
            Self {
                inner: Box::pin(stream.map(Ok)),
            }
        }

        /// Read the next chunk from the stream.
        pub async fn next(&mut self) -> Option<Result<Bytes, StreamingError>> {
            self.inner.as_mut().next().await
        }
    }

    /// Re-export [`bytes::Bytes`] so that call-sites can use
    /// `crate::fullstack_compat::body::Bytes` regardless of the feature flag.
    pub mod body {
        pub use bytes::Bytes;
    }
}

#[cfg(feature = "fullstack")]
pub(crate) mod inner {
    //! Re-export directly from the real dioxus-fullstack crate.
    pub use dioxus::fullstack::body;
    pub use dioxus::fullstack::{ByteStream, StreamingError};
}

// ---------------------------------------------------------------------------
// Convenience re-exports – other modules use `crate::fullstack_compat::*`
// ---------------------------------------------------------------------------

pub use inner::*;

/// Collect all bytes from a [`ByteStream`] into a single `Vec<u8>`.
///
/// Works with both the real `dioxus::fullstack::ByteStream` and the local
/// replacement used when `fullstack` is not enabled.
pub async fn collect_bytes_from_stream(mut stream: ByteStream) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(data) => bytes.extend_from_slice(&data),
            Err(e) => return Err(format!("Stream read error: {e}")),
        }
    }
    Ok(bytes)
}
