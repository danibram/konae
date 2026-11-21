pub mod artifact;
pub mod domain;
pub mod hostname;
pub mod hosts;
pub mod page;
pub mod path;
pub mod path_name;
pub mod seo;

mod serving;
mod deployment_host;
mod address;

pub use serving::{serve_root, login_page, serve_artifact, serve_key, count_page_view, read_artifact, private_login_page, ArtifactCache, etag_matches, artifact_response};
pub use deployment_host::{deployment_for, deployment_for_host, deployment_for_request};
pub use address::{deployment_url, safe_slug, content_type};
pub(crate) use serving::serve_key_with_routes;
