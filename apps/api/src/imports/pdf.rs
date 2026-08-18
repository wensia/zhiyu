use std::{
    fs::{OpenOptions, remove_file},
    io::Write,
    process::Command,
    sync::OnceLock,
};

use regex::Regex;
use uuid::Uuid;

use super::model::{ImportParseError, MAX_IMPORT_BYTES};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Word {
    pub page: usize,
    pub x_min: f64,
    pub y_min: f64,
    pub y_max: f64,
    pub text: String,
}

impl Word {
    pub(crate) fn y_center(&self) -> f64 {
        (self.y_min + self.y_max) / 2.0
    }
}

struct TemporaryPdf(std::path::PathBuf);

impl Drop for TemporaryPdf {
    fn drop(&mut self) {
        let _ = remove_file(&self.0);
    }
}

pub(crate) fn extract_words(bytes: &[u8]) -> Result<Vec<Word>, ImportParseError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(ImportParseError::new(
            "import_resource_limit",
            format!("文件超过 {MAX_IMPORT_BYTES} 字节"),
        ));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(ImportParseError::new(
            "invalid_import_pdf",
            "文件不是有效的 PDF",
        ));
    }

    let path = std::env::temp_dir().join(format!("zhiyu-pdf-import-{}.pdf", Uuid::now_v7()));
    let temporary = TemporaryPdf(path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    // 临时文件装的是用户的真实银行账单（姓名、卡号、完整流水）。默认 0644 会让同机
    // 其他用户可读，落盘期间必须收窄到仅属主。
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary.0).map_err(|error| {
        ImportParseError::new(
            "pdf_extract_failed",
            format!("无法创建 PDF 临时文件: {error}"),
        )
    })?;
    file.write_all(bytes).map_err(|error| {
        ImportParseError::new(
            "pdf_extract_failed",
            format!("无法写入 PDF 临时文件: {error}"),
        )
    })?;
    drop(file);

    let output = Command::new("pdftotext")
        .arg("-bbox-layout")
        .arg(&temporary.0)
        .arg("-")
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ImportParseError::new(
                    "missing_pdf_extractor",
                    "系统缺少 pdftotext；请安装 poppler-utils 后重试",
                )
            } else {
                ImportParseError::new("pdf_extract_failed", format!("无法启动 pdftotext: {error}"))
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let diagnostic = stderr.lines().next().unwrap_or("未知错误").trim();
        return Err(ImportParseError::new(
            "pdf_extract_failed",
            format!("pdftotext 提取失败: {diagnostic}"),
        ));
    }
    let xhtml = String::from_utf8(output.stdout).map_err(|_| {
        ImportParseError::new("pdf_extract_failed", "pdftotext 输出不是有效的 UTF-8")
    })?;
    parse_bbox_xhtml(&xhtml)
}

pub(crate) fn parse_bbox_xhtml(xhtml: &str) -> Result<Vec<Word>, ImportParseError> {
    static WORD_RE: OnceLock<Regex> = OnceLock::new();
    let word_re = WORD_RE.get_or_init(|| {
        Regex::new(
            r#"<word\s+xMin=\"([^\"]+)\"\s+yMin=\"([^\"]+)\"\s+xMax=\"[^\"]+\"\s+yMax=\"([^\"]+)\"[^>]*>(.*?)</word>"#,
        )
        .expect("valid bbox word regex")
    });
    let mut page = 0usize;
    let mut words = Vec::new();
    for line in xhtml.lines() {
        if line.contains("<page ") {
            page += 1;
        }
        let Some(captures) = word_re.captures(line) else {
            continue;
        };
        if page == 0 {
            return Err(ImportParseError::new(
                "invalid_import_pdf",
                "PDF bbox 输出中的 word 缺少 page",
            ));
        }
        let coordinate = |index: usize, name: &str| {
            captures[index].parse::<f64>().map_err(|_| {
                ImportParseError::new(
                    "invalid_import_pdf",
                    format!("PDF bbox 输出包含无效坐标 {name}"),
                )
            })
        };
        words.push(Word {
            page,
            x_min: coordinate(1, "xMin")?,
            y_min: coordinate(2, "yMin")?,
            y_max: coordinate(3, "yMax")?,
            text: decode_xml_text(&captures[4])?,
        });
    }
    if words.is_empty() {
        return Err(ImportParseError::new(
            "empty_import_file",
            "PDF 中没有可提取的文字",
        ));
    }
    Ok(words)
}

fn decode_xml_text(value: &str) -> Result<String, ImportParseError> {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        result.push_str(&rest[..index]);
        let entity = &rest[index..];
        let Some(end) = entity.find(';') else {
            return Err(ImportParseError::new(
                "invalid_import_pdf",
                "PDF bbox 输出包含未闭合的 XML 实体",
            ));
        };
        let name = &entity[1..end];
        let decoded = match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ if name.starts_with("#x") => u32::from_str_radix(&name[2..], 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(invalid_entity)?,
            _ if name.starts_with('#') => name[1..]
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(invalid_entity)?,
            _ => return Err(invalid_entity()),
        };
        result.push(decoded);
        rest = &entity[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

fn invalid_entity() -> ImportParseError {
    ImportParseError::new("invalid_import_pdf", "PDF bbox 输出包含无效的 XML 实体")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bbox_words_and_entities() {
        let xhtml = r#"<page width="100" height="100">
<word xMin="36.000000" yMin="43.633604" xMax="75.000000" yMax="53.383604">A&amp;B</word>
</page>"#;
        let words = parse_bbox_xhtml(xhtml).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].page, 1);
        assert_eq!(words[0].text, "A&B");
        assert_eq!(words[0].x_min, 36.0);
    }
}
