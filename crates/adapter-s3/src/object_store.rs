use std::fmt::Display;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use app::domain::address::Address;
use app::domain::repository::RepositoryId;
use app::port::driven::object_store::{Grant, GrantMethod, ObjectStore, ObjectStoreError};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::{Client, Response, StatusCode};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

/// How long a grant lasts unless [`S3ObjectStore::with_expiry`] says otherwise.
pub const DEFAULT_EXPIRY: Duration = Duration::from_secs(15 * 60);

/// The largest object S3 accepts in one `PUT`, in bytes (5 GB, ADR 0023).
pub const SINGLE_PUT_LIMIT: u64 = 5 * 1024 * 1024 * 1024;

/// Objects in one bucket of an S3-compatible store, each under `<repository id>/<sha256 hex>`.
/// The store addresses the bucket by path.
pub struct S3ObjectStore {
    bucket: Bucket,
    credentials: Credentials,
    client: Client,
    expiry: Duration,
    max_upload_size: u64,
}

fn backend(failure: impl Display) -> ObjectStoreError {
    ObjectStoreError::Backend(failure.to_string())
}

/// The request's failure without its URL, which carries the signature.
fn transport(failure: reqwest::Error) -> ObjectStoreError {
    backend(failure.without_url())
}

fn status(response: &Response) -> ObjectStoreError {
    backend(format!("the store answered {}", response.status()))
}

/// The address's hex digits, which follow its algorithm prefix.
fn hex(address: Address) -> String {
    let text = address.to_string();
    text.strip_prefix("sha256:").unwrap_or(&text).to_owned()
}

/// The base64 of the address's digest, the form `x-amz-checksum-sha256` takes.
fn checksum(address: Address) -> Result<String, ObjectStoreError> {
    let hex = hex(address);
    let digest = (0..hex.len())
        .step_by(2)
        .map(|start| {
            hex.get(start..start + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(|| backend(format!("address {address} has no digest")))?;
    Ok(STANDARD.encode(digest))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

impl S3ObjectStore {
    /// A store over `bucket` at `endpoint`, refusing uploads above `max_upload_size` bytes.
    ///
    /// # Errors
    ///
    /// [`ObjectStoreError::Backend`] where `endpoint` is not an `http` or `https` URL, or
    /// `max_upload_size` exceeds [`SINGLE_PUT_LIMIT`].
    pub fn new(
        endpoint: &str,
        region: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        max_upload_size: u64,
    ) -> Result<Self, ObjectStoreError> {
        if max_upload_size > SINGLE_PUT_LIMIT {
            return Err(backend(format!(
                "maximum upload size {max_upload_size} exceeds the single PUT limit {SINGLE_PUT_LIMIT}"
            )));
        }
        let bucket = Bucket::new(
            endpoint.parse().map_err(backend)?,
            UrlStyle::Path,
            bucket.to_owned(),
            region.to_owned(),
        )
        .map_err(backend)?;
        Ok(Self {
            bucket,
            credentials: Credentials::new(access_key, secret_key),
            client: Client::new(),
            expiry: DEFAULT_EXPIRY,
            max_upload_size,
        })
    }

    /// This store with grants that last `expiry`.
    #[must_use]
    pub const fn with_expiry(mut self, expiry: Duration) -> Self {
        self.expiry = expiry;
        self
    }

    fn key(repository: RepositoryId, address: Address) -> String {
        format!("{repository}/{}", hex(address))
    }

    fn grant(
        &self,
        method: GrantMethod,
        url: impl Display,
        headers: Vec<(String, String)>,
        issued_at: i64,
    ) -> Grant {
        let lifetime = i64::try_from(self.expiry.as_secs()).unwrap_or(i64::MAX);
        Grant {
            url: url.to_string(),
            method,
            headers,
            expires_at: issued_at.saturating_add(lifetime),
        }
    }

    async fn send(
        &self,
        grant: &Grant,
        body: Option<Vec<u8>>,
    ) -> Result<Response, ObjectStoreError> {
        let mut request = match grant.method {
            GrantMethod::Get => self.client.get(&grant.url),
            GrantMethod::Put => self.client.put(&grant.url),
        };
        for (name, value) in &grant.headers {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        request.send().await.map_err(transport)
    }
}

impl ObjectStore for S3ObjectStore {
    async fn put(
        &self,
        repository: RepositoryId,
        bytes: Vec<u8>,
    ) -> Result<Address, ObjectStoreError> {
        let address = Address::of(&bytes);
        let size = u64::try_from(bytes.len()).map_err(backend)?;
        let grant = self.upload_grant(repository, address, size).await?;
        let response = self.send(&grant, Some(bytes)).await?;
        if response.status().is_success() {
            Ok(address)
        } else {
            Err(status(&response))
        }
    }

    async fn get(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Vec<u8>, ObjectStoreError> {
        let grant = self.download_grant(repository, address).await?;
        let response = self.send(&grant, None).await?;
        match response.status() {
            StatusCode::NOT_FOUND => Err(ObjectStoreError::NotFound(address)),
            code if code.is_success() => Ok(response.bytes().await.map_err(transport)?.to_vec()),
            _ => Err(status(&response)),
        }
    }

    async fn exists(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<bool, ObjectStoreError> {
        let key = Self::key(repository, address);
        let url = self
            .bucket
            .head_object(Some(&self.credentials), &key)
            .sign(self.expiry);
        let response = self.client.head(url).send().await.map_err(transport)?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(false),
            code if code.is_success() => Ok(true),
            _ => Err(status(&response)),
        }
    }

    /// Signs the object's size and checksum into the URL, so the store takes only bytes of that
    /// size that hash to `address`. A `size` above the configured maximum is refused as
    /// [`ObjectStoreError::Backend`], since the port has no variant for it.
    async fn upload_grant(
        &self,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> Result<Grant, ObjectStoreError> {
        if size > self.max_upload_size {
            return Err(backend(format!(
                "object of {size} bytes exceeds the maximum upload size {}",
                self.max_upload_size
            )));
        }
        let issued_at = now();
        let checksum = checksum(address)?;
        let key = Self::key(repository, address);
        let mut action = self.bucket.put_object(Some(&self.credentials), &key);
        action
            .headers_mut()
            .insert("content-length", size.to_string());
        action
            .headers_mut()
            .insert("x-amz-checksum-sha256", checksum.clone());
        let url = action.sign(self.expiry);
        Ok(self.grant(
            GrantMethod::Put,
            url,
            vec![
                ("content-length".to_owned(), size.to_string()),
                ("x-amz-checksum-sha256".to_owned(), checksum),
            ],
            issued_at,
        ))
    }

    async fn download_grant(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Grant, ObjectStoreError> {
        let issued_at = now();
        let key = Self::key(repository, address);
        let url = self
            .bucket
            .get_object(Some(&self.credentials), &key)
            .sign(self.expiry);
        Ok(self.grant(GrantMethod::Get, url, Vec::new(), issued_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_digest_as_base64() {
        assert_eq!(
            checksum(Address::of(b"hello world")).unwrap(),
            "uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek="
        );
    }

    #[test]
    fn keys_an_object_under_its_repository_and_hex_digest() {
        assert_eq!(
            S3ObjectStore::key(RepositoryId::new(7), Address::of(b"hello world")),
            "7/b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn refuses_a_maximum_above_the_single_put_limit() {
        let refused = S3ObjectStore::new(
            "http://127.0.0.1:1",
            "us-east-1",
            "b",
            "k",
            "s",
            SINGLE_PUT_LIMIT + 1,
        );
        assert!(refused.is_err());
    }
}
