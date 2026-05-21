use super::RuntimeResult;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// A semantic version that can be compared
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Version(String);

impl Version {
    /// Create a new version from a string
    pub fn new(version: impl Into<String>) -> Self {
        Version(version.into())
    }

    /// Compare two versions semantically
    pub fn compare(&self, other: &Version) -> Ordering {
        let self_parts: Vec<u32> = self
            .0
            .split('.')
            .take(3)
            .filter_map(|p| p.parse().ok())
            .collect();
        let other_parts: Vec<u32> = other
            .0
            .split('.')
            .take(3)
            .filter_map(|p| p.parse().ok())
            .collect();

        for i in 0..3 {
            let self_part = self_parts.get(i).copied().unwrap_or(0);
            let other_part = other_parts.get(i).copied().unwrap_or(0);

            match self_part.cmp(&other_part) {
                Ordering::Equal => continue,
                other => return other,
            }
        }

        Ordering::Equal
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare(other)
    }
}

/// Image digest (SHA256 hash)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ImageDigest(String);

impl ImageDigest {
    /// Create a new digest from a string
    pub fn new(digest: impl Into<String>) -> Self {
        ImageDigest(digest.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ImageDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Complete image tag identifier (e.g., "library/alpine:3.18", "myrepo/colmap:v1.0.0")
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ImageTag(String);

impl ImageTag {
    /// Create a new image tag from repository:version format
    pub fn new(repository: impl Into<String>, tag: impl Into<String>) -> Self {
        let repo = repository.into();
        let tag_str = tag.into();
        ImageTag(format!("{}:{}", repo, tag_str))
    }

    /// Parse tag from "repository:version" format
    pub fn from_string(full_tag: impl Into<String>) -> Self {
        ImageTag(full_tag.into())
    }

    /// Get the full tag string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Split tag into (repository, version)
    pub fn parts(&self) -> (&str, &str) {
        match self.0.rsplit_once(':') {
            Some((repo, tag)) => (repo, tag),
            None => (self.0.as_str(), "latest"),
        }
    }

    /// Get repository part (before the colon)
    pub fn repository(&self) -> &str {
        self.parts().0
    }

    /// Get version part (after the colon)
    pub fn version(&self) -> Version {
        Version::new(self.parts().1.to_string())
    }
}

impl fmt::Display for ImageTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ImageTag {
    fn from(s: String) -> Self {
        ImageTag(s)
    }
}

impl From<&str> for ImageTag {
    fn from(s: &str) -> Self {
        ImageTag(s.to_string())
    }
}

/// Remote image information from a registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteImage {
    /// Complete image tag (repository:version)
    pub tag: ImageTag,
    /// Content digest/hash of this image
    pub digest: Option<ImageDigest>,
    /// Size in bytes
    pub size: Option<u64>,
    /// When the image was pushed
    pub created: Option<String>,
}

impl RemoteImage {
    /// Create a new remote image
    pub fn new(repository: impl Into<String>, tag: impl Into<String>) -> Self {
        RemoteImage {
            tag: ImageTag::new(repository, tag),
            digest: None,
            size: None,
            created: None,
        }
    }

    /// Create a remote image with all fields
    pub fn with_digest(
        repository: impl Into<String>,
        tag: impl Into<String>,
        digest: Option<ImageDigest>,
    ) -> Self {
        RemoteImage {
            tag: ImageTag::new(repository, tag),
            digest,
            size: None,
            created: None,
        }
    }

    /// Check if this remote image is newer than another version
    pub fn is_newer_than(&self, other: &Version) -> bool {
        self.tag.version().compare(other) == Ordering::Greater
    }

    /// Get the repository name
    pub fn repository(&self) -> &str {
        self.tag.repository()
    }

    /// Get the version
    pub fn version(&self) -> Version {
        self.tag.version()
    }
}

/// Docker Registry V2 API client
pub struct RegistryClient {
    registry_url: String,
    client: reqwest::Client,
}

impl RegistryClient {
    /// Create a new registry client pointing to Docker Hub
    pub fn docker_hub() -> Self {
        Self::new("https://registry.hub.docker.com".to_string())
    }

    /// Create a new registry client for a custom registry
    pub fn new(registry_url: String) -> Self {
        RegistryClient {
            registry_url: registry_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// List all tags for a repository in the registry
    pub async fn list_tags(&self, repository: &str) -> RuntimeResult<Vec<RemoteImage>> {
        // For Docker Hub, use the special v2 API
        let url = if self.registry_url.contains("registry.hub.docker.com")
            || self.registry_url.contains("docker.io")
        {
            format!(
                "https://registry.hub.docker.com/v2/repositories/library/{}/tags",
                repository
            )
        } else {
            format!("{}/v2/{}/tags/list", self.registry_url, repository)
        };

        let response = self
            .client
            .get(&url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch tags: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Registry returned status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ));
        }

        // Handle Docker Hub's different response format
        if self.registry_url.contains("registry.hub.docker.com")
            || self.registry_url.contains("docker.io")
        {
            #[derive(Deserialize)]
            struct DockerHubTag {
                name: String,
            }

            #[derive(Deserialize)]
            struct DockerHubResponse {
                results: Vec<DockerHubTag>,
            }

            let data: DockerHubResponse = response
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

            let images = data
                .results
                .into_iter()
                .map(|tag| RemoteImage::new(repository, tag.name))
                .collect();

            Ok(images)
        } else {
            // Standard Docker Registry V2 API
            #[derive(Deserialize)]
            struct RegistryResponse {
                tags: Vec<String>,
            }

            let data: RegistryResponse = response
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

            let images = data
                .tags
                .into_iter()
                .map(|tag| RemoteImage::new(repository, tag))
                .collect();

            Ok(images)
        }
    }

    /// Get manifest for a specific image tag
    pub async fn get_manifest(&self, tag: &ImageTag) -> RuntimeResult<String> {
        let (repo, tag_str) = tag.parts();
        let url = format!("{}/v2/{}/manifests/{}", self.registry_url, repo, tag_str);

        let response = self
            .client
            .get(&url)
            .header(
                "Accept",
                "application/vnd.docker.distribution.manifest.v2+json",
            )
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch manifest: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to get manifest: {}",
                response.status()
            ));
        }

        response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read manifest: {}", e))
    }

    /// Get digest for a specific image tag
    pub async fn get_digest(&self, tag: &ImageTag) -> RuntimeResult<ImageDigest> {
        let (repo, tag_str) = tag.parts();
        let url = format!("{}/v2/{}/manifests/{}", self.registry_url, repo, tag_str);

        let response = self
            .client
            .head(&url)
            .header(
                "Accept",
                "application/vnd.docker.distribution.manifest.v2+json",
            )
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch manifest: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to get digest: {}",
                response.status()
            ));
        }

        response
            .headers()
            .get("Docker-Content-Digest")
            .and_then(|h| h.to_str().ok())
            .map(ImageDigest::new)
            .ok_or_else(|| anyhow::anyhow!("No digest found in response"))
    }

    /// Search for images in the registry
    pub async fn search(&self, query: &str, limit: usize) -> RuntimeResult<Vec<RemoteImage>> {
        // Docker Hub search API
        if self.registry_url.contains("registry.hub.docker.com")
            || self.registry_url.contains("docker.io")
        {
            let url = format!(
                "https://hub.docker.com/v2/search/repositories?query={}&limit={}",
                query, limit
            );

            let response = self
                .client
                .get(&url)
                .header("User-Agent", "colmap-openmvs-app")
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to search: {}", e))?;

            #[derive(Deserialize)]
            struct SearchResult {
                name: String,
            }

            #[derive(Deserialize)]
            struct SearchResponse {
                results: Vec<SearchResult>,
            }

            let data: SearchResponse = response
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to parse search results: {}", e))?;

            let mut images = Vec::new();
            for result in data.results.iter().take(limit) {
                if let Ok(tags) = self.list_tags(&result.name).await {
                    images.extend(tags);
                }
            }

            Ok(images)
        } else {
            Err(anyhow::anyhow!("Search is only supported for Docker Hub"))
        }
    }
}

/// Update information for an installed image
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpdateInfo {
    /// The image tag that is currently installed
    pub installed: ImageTag,
    /// The latest available version from the registry
    pub latest: RemoteImage,
    /// Whether an update is available
    pub update_available: bool,
}

impl UpdateInfo {
    /// Create update info by comparing installed with available versions
    pub fn new(installed: ImageTag, available_images: &[RemoteImage]) -> Option<Self> {
        let latest = available_images
            .iter()
            .max_by(|a, b| a.version().compare(&b.version()))
            .cloned()?;

        let update_available = latest.is_newer_than(&installed.version());

        Some(UpdateInfo {
            installed,
            latest,
            update_available,
        })
    }

    /// Get a human-readable message about the update status
    pub fn status_message(&self) -> String {
        if self.update_available {
            format!(
                "Update available for {}: {} → {}",
                self.installed.repository(),
                self.installed.version(),
                self.latest.version()
            )
        } else {
            format!(
                "{} is up to date ({})",
                self.installed.repository(),
                self.installed.version()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        let v1 = Version::new("1.0.0");
        let v2 = Version::new("1.1.0");
        let v3 = Version::new("0.9.0");

        assert!(v1.compare(&v2) == Ordering::Less);
        assert!(v2.compare(&v1) == Ordering::Greater);
        assert!(v1.compare(&v3) == Ordering::Greater);
        assert!(v1.compare(&v1) == Ordering::Equal);
    }

    #[test]
    fn test_image_tag_creation() {
        let tag = ImageTag::new("alpine", "3.18");
        assert_eq!(tag.as_str(), "alpine:3.18");
        assert_eq!(tag.repository(), "alpine");
        assert_eq!(tag.version().as_str(), "3.18");
    }

    #[test]
    fn test_image_tag_parts() {
        let tag = ImageTag::from_string("library/ubuntu:22.04");
        assert_eq!(tag.repository(), "library/ubuntu");
        assert_eq!(tag.version().as_str(), "22.04");
    }

    #[test]
    fn test_image_digest() {
        let digest = ImageDigest::new(
            "sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        );
        assert!(digest.as_str().starts_with("sha256:"));
    }

    #[test]
    fn test_remote_image_newer_than() {
        let img = RemoteImage::new("alpine", "3.18");
        assert!(img.is_newer_than(&Version::new("3.17")));
        assert!(!img.is_newer_than(&Version::new("3.18")));
        assert!(!img.is_newer_than(&Version::new("3.19")));
    }

    #[test]
    fn test_update_info() {
        let installed = ImageTag::new("ubuntu", "20.04");
        let available = vec![
            RemoteImage::new("ubuntu", "20.04"),
            RemoteImage::new("ubuntu", "22.04"),
        ];

        let info = UpdateInfo::new(installed, &available).unwrap();
        assert!(info.update_available);
        assert_eq!(info.latest.version().as_str(), "22.04");
    }
}
