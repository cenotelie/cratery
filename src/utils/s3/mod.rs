/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! API to access S3 buckets and objects

pub mod signing;

use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Body;
use serde_derive::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::{AsyncRead, BufReader, ReadBuf};

use crate::utils::apierror::ApiError;

/// The S3 parameters
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct S3Params {
    /// The base URI for S3
    pub uri: String,
    /// The region to target
    pub region: String,
    /// The service name in the URI, if any
    pub service: Option<String>,
    /// The account access key
    #[serde(rename = "accessKey")]
    pub access_key: String,
    /// The account secret key
    #[serde(rename = "secretKey")]
    pub secret_key: String,
}

impl S3Params {
    /// Gets the final service URI
    #[must_use]
    pub fn service_uri(&self) -> String {
        match &self.service {
            None => format!("{}.{}", &self.region, &self.uri),
            Some(service) => format!("{}.{}.{}", service, &self.region, &self.uri),
        }
    }

    /// Gets the URI for the bucket
    #[must_use]
    pub fn bucket_uri(&self, bucket: &str) -> String {
        match &self.service {
            None => format!("{}.{}.{}", bucket, &self.region, &self.uri),
            Some(service) => format!("{}.{}.{}.{}", bucket, service, &self.region, &self.uri),
        }
    }
}

/// Gets all known buckets
///
/// # Errors
///
/// Return an `ApiError` when the request fails
pub async fn list_all_buckets(params: &S3Params) -> Result<Vec<String>, ApiError> {
    let target = params.service_uri();
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::HOST, HeaderValue::from_str(&target).unwrap());
    signing::sign_request(params, "GET", "/", &[], &mut headers, &signing::sha256(b""));
    let response = reqwest::Client::default()
        .get(format!("https://{target}/"))
        .headers(headers)
        .send()
        .await?;
    let status = response.status();
    let content = response.bytes().await?;
    let content = String::from_utf8(content.to_vec())?;
    if status.is_success() {
        let mut reader = Reader::from_str(&content);
        reader.config_mut().trim_text(true);
        let mut in_name = false;
        let mut names = Vec::new();
        loop {
            match reader.read_event()? {
                Event::Start(e) if e.name().0 == b"Name" => {
                    in_name = true;
                }
                Event::End(e) if e.name().0 == b"Name" => {
                    in_name = false;
                }
                Event::Text(e) => {
                    if in_name {
                        names.push(e.unescape()?.to_string());
                    }
                }
                Event::Eof => break,
                _ => (),
            }
        }
        Ok(names)
    } else {
        Err(ApiError::new(status.as_u16(), content, None))
    }
}

/// Creates a new S3 bucket
///
/// # Errors
///
/// Return an `ApiError` when the request fails
pub async fn create_bucket(params: &S3Params, name: &str) -> Result<(), ApiError> {
    let content = format!(
        "<CreateBucketConfiguration><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>",
        &params.region
    );
    let content_bytes = content.as_bytes().to_vec();
    let content_length = content_bytes.len();

    let target = params.bucket_uri(name);
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::HOST, HeaderValue::from_str(&target).unwrap());
    signing::sign_request(params, "PUT", "/", &[], &mut headers, "UNSIGNED-PAYLOAD");
    headers.insert(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_str(&content_length.to_string()).unwrap(),
    );
    headers.insert(reqwest::header::CONTENT_TYPE, HeaderValue::from_static("application/xml"));

    let response = reqwest::Client::default()
        .put(format!("https://{target}/"))
        .headers(headers)
        .body(content_bytes)
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let content = response.bytes().await?;
        let content = String::from_utf8(content.to_vec())?;
        Err(ApiError::new(status.as_u16(), content, None))
    }
}

/// Uploads an S3 object
///
/// # Errors
///
/// Return an `ApiError` when the request fails
pub async fn upload_object_raw(params: &S3Params, bucket: &str, object: &str, content: Vec<u8>) -> Result<usize, ApiError> {
    let length = content.len();
    let target = params.bucket_uri(bucket);
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::HOST, HeaderValue::from_str(&target).unwrap());
    let path = format!("/{object}");
    signing::sign_request(params, "PUT", &path, &[], &mut headers, "UNSIGNED-PAYLOAD");
    headers.insert(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap(),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    let response = reqwest::Client::default()
        .put(format!("https://{}{}", &target, &path))
        .headers(headers)
        .body(content)
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        Ok(length)
    } else {
        let content = response.bytes().await?;
        let content = String::from_utf8(content.to_vec())?;
        Err(ApiError::new(status.as_u16(), content, None))
    }
}

/// Downloads an S3 object
///
/// # Errors
///
/// Return an `ApiError` when the request fails
pub async fn get_object(params: &S3Params, bucket: &str, object: &str) -> Result<Vec<u8>, ApiError> {
    let content_hash = signing::sha256(b"");
    let target = params.bucket_uri(bucket);
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::HOST, HeaderValue::from_str(&target).unwrap());
    let path = format!("/{object}");
    signing::sign_request(params, "GET", &path, &[], &mut headers, &content_hash);
    let response = reqwest::Client::default()
        .get(format!("https://{}{}", &target, &path))
        .headers(headers)
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        while let Some(bytes) = stream.next().await {
            let bytes = bytes?;
            buffer.extend_from_slice(&bytes);
        }
        Ok(buffer)
    } else {
        Err(ApiError::new(status.as_u16(), String::new(), None))
    }
}

/// Uploads an S3 object
///
/// # Errors
///
/// Return an `ApiError` when the request fails
pub async fn upload_object_file<P: AsRef<Path>>(
    params: &S3Params,
    bucket: &str,
    object: &str,
    path: P,
) -> Result<(), ApiError> {
    let file = tokio::fs::File::open(path).await?;
    let metadata = file.metadata().await?;
    let reader = tokio::io::BufReader::new(file);
    upload_object_stream(params, bucket, object, TokioFileAdapter::wrap(reader), metadata.len()).await
}

/// Uploads an S3 object
///
/// # Errors
///
/// Return an `ApiError` when the request fails
pub async fn upload_object_stream<S, O, E>(
    params: &S3Params,
    bucket: &str,
    object: &str,
    content: S,
    content_length: u64,
) -> Result<(), ApiError>
where
    S: Stream<Item = Result<O, E>> + Send + Sync + 'static,
    Bytes: From<O>,
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    let target = params.bucket_uri(bucket);
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::HOST, HeaderValue::from_str(&target).unwrap());
    let path = format!("/{object}");
    signing::sign_request(params, "PUT", &path, &[], &mut headers, "UNSIGNED-PAYLOAD");
    headers.insert(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_str(&content_length.to_string()).unwrap(),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    let response = reqwest::Client::default()
        .put(format!("https://{}{}", &target, &path))
        .headers(headers)
        .body(Body::wrap_stream(content))
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let content = response.bytes().await?;
        let content = String::from_utf8(content.to_vec())?;
        Err(ApiError::new(status.as_u16(), content, None))
    }
}

/// Wraps a tokio file reader as a stream of byte buffers
struct TokioFileAdapter {
    /// The underlying reader
    reader: BufReader<File>,
    /// The buffer to fill
    buffer: Vec<u8>,
    /// The current index to start writing new data into the buffer
    current: usize,
    /// Whether EOF was reached
    at_end: bool,
}

const BUFFER_SIZE: usize = 4096;

impl TokioFileAdapter {
    /// Wraps a reader into this adapter
    fn wrap(reader: BufReader<File>) -> Self {
        Self {
            reader,
            buffer: Self::new_buffer(),
            current: 0,
            at_end: false,
        }
    }

    /// Creates a new empty, initialized buffer
    fn new_buffer() -> Vec<u8> {
        vec![0; BUFFER_SIZE]
    }

    /// Swaps the current buffer for a new one and get back the current one
    fn swap_buffer(&mut self) -> Vec<u8> {
        let mut buffer = Self::new_buffer();
        std::mem::swap(&mut self.buffer, &mut buffer);
        self.current = 0;
        buffer
    }
}

impl Stream for TokioFileAdapter {
    type Item = Result<Vec<u8>, tokio::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.at_end {
            return Poll::Ready(None);
        }
        let this = self.get_mut();
        let mut cursor = ReadBuf::new(&mut this.buffer[this.current..]);
        let reader = Pin::new(&mut this.reader);
        match reader.poll_read(cx, &mut cursor) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Ready(Ok(())) => {
                let read_bytes = cursor.filled().len();
                if read_bytes == 0 {
                    // at end
                    this.at_end = true;
                    if this.current == 0 {
                        // no more bytes
                        Poll::Ready(None)
                    } else {
                        // last bytes
                        Poll::Ready(Some(Ok(this.swap_buffer())))
                    }
                } else {
                    // not at end
                    this.current += read_bytes;
                    if this.current == BUFFER_SIZE {
                        // buffer is full, return it
                        Poll::Ready(Some(Ok(this.swap_buffer())))
                    } else {
                        // wait for a full buffer
                        Poll::Pending
                    }
                }
            }
        }
    }
}
