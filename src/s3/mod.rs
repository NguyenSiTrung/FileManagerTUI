/// S3 browse mode module.
///
/// Provides read-only browsing of AWS S3 buckets by shelling out
/// to the `aws` CLI. No AWS SDK dependency.
pub mod backend;
pub mod parser;
pub mod types;

pub use backend::S3Backend;
pub use types::{S3Config, S3Entry, S3Path};
