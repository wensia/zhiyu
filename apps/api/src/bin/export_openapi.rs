use std::{env, fs};

use anyhow::{Context, Result};
use utoipa::OpenApi;
use zhiyu_api::ApiDoc;

fn main() -> Result<()> {
    let path = env::args().nth(1).context("usage: export_openapi <path>")?;
    let json = ApiDoc::openapi().to_pretty_json()?;
    if let Some(parent) = std::path::Path::new(&path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, json)?;
    Ok(())
}
