/*******************************************************************************
 * Copyright (c) 2022 Cénotélie Opérations SAS (cenotelie.fr)
******************************************************************************/

//! Serving embedded resources

use std::collections::HashMap;

/// The data for an embedded resource
#[derive(Debug, Clone)]
pub struct Resource {
    /// The resource's filename
    #[allow(unused)]
    pub file_name: &'static str,
    /// The content type for the resource
    pub content_type: &'static str,
    /// The content of the resource
    pub content: &'static [u8],
}

/// A registry of embedded resources
#[derive(Debug, Default, Clone)]
pub struct Resources {
    /// The known resources
    pub data: HashMap<&'static str, Resource>,
    /// Path to the fallback resource, if any
    pub fallback: &'static str,
}

impl Resources {
    /// Creates an empty registry with the specified fallback
    #[must_use]
    pub fn with_fallback(fallback: &'static str) -> Self {
        Self {
            data: HashMap::new(),
            fallback,
        }
    }

    /// Gets the resource at the specified path
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&Resource> {
        self.data.get(path).or_else(|| self.data.get(self.fallback))
    }
}

/// Gets the content type for a file
pub fn get_content_type(path: &str) -> &'static str {
    let extension = path.rfind('.').map(|index| &path[(index + 1)..]);
    match extension {
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("gif") => "image/gif",
        Some("png") => "image/png",
        Some("jpeg") => "image/jpeg",
        Some("bmp") => "image/bmp",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}
