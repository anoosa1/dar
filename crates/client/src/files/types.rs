//! File type tiles and labels (`file-types.tsx`).

use leptos::prelude::*;

use crate::icons::Icon;

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

fn ends_with_any(name: &str, extensions: &[&str]) -> bool {
    name.contains('.') && extensions.contains(&extension(name).as_str())
}

/// Classifies an entry by kind, media type and then extension.
pub fn file_type(kind: &str, name: &str, media: &str) -> &'static str {
    if kind == "folder" {
        return "folder";
    }
    if media.starts_with("image/") {
        return "image";
    }
    if media.starts_with("audio/") {
        return "audio";
    }
    if media.starts_with("video/") {
        return "video";
    }
    if media == "application/pdf" {
        return "pdf";
    }
    if ["zip", "x-tar", "gzip", "x-7z", "x-rar", "x-xz", "x-bzip"]
        .iter()
        .any(|m| media.contains(m))
    {
        return "archive";
    }
    if ["csv", "spreadsheet", "ms-excel"]
        .iter()
        .any(|m| media.contains(m))
    {
        return "sheet";
    }
    let table: [(&str, &[&str]); 8] = [
        ("sheet", &["csv", "tsv", "xls", "xlsx", "ods", "numbers"]),
        ("pdf", &["pdf"]),
        (
            "image",
            &[
                "png", "jpg", "jpeg", "gif", "webp", "svg", "heic", "avif", "bmp", "tif", "tiff",
            ],
        ),
        (
            "archive",
            &["zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst"],
        ),
        (
            "audio",
            &["mp3", "m4a", "ogg", "oga", "flac", "wav", "opus", "aac"],
        ),
        ("video", &["mp4", "m4v", "mov", "webm", "mkv", "avi"]),
        (
            "code",
            &[
                "json", "yml", "yaml", "toml", "xml", "htm", "html", "css", "js", "jsx", "ts",
                "tsx", "cjs", "mjs", "cts", "mts", "cjsx", "mjsx", "ctsx", "mtsx", "py", "rb",
                "go", "rs", "java", "c", "h", "cpp", "sh", "nix", "sql", "ini", "conf",
            ],
        ),
        (
            "doc",
            &[
                "md", "markdown", "txt", "rtf", "doc", "docx", "odt", "pages", "tex", "org",
            ],
        ),
    ];
    if let Some((kind, _)) = table.iter().find(|(_, list)| ends_with_any(name, list)) {
        return kind;
    }
    if media.starts_with("text/") {
        "doc"
    } else if ["json", "xml", "javascript"]
        .iter()
        .any(|m| media.contains(m))
    {
        "code"
    } else {
        "other"
    }
}

pub fn file_type_label(kind: &str, name: &str, media: &str) -> &'static str {
    let ty = file_type(kind, name, media);
    if ty == "doc" && ends_with_any(name, &["md", "markdown"]) {
        return "Markdown";
    }
    if ty == "doc" && ends_with_any(name, &["txt"]) {
        return "Text";
    }
    match ty {
        "folder" => "Folder",
        "doc" => "Document",
        "sheet" => "Spreadsheet",
        "image" => "Image",
        "pdf" => "PDF",
        "code" => "Code",
        "archive" => "Archive",
        "audio" => "Audio",
        "video" => "Video",
        _ => "File",
    }
}

fn icon_for(ty: &str) -> &'static str {
    match ty {
        "folder" => "folder",
        "sheet" => "sheet",
        "image" => "image",
        "pdf" => "pdf",
        "code" => "code",
        "archive" => "archive2",
        "audio" => "media",
        "video" => "video",
        _ => "file",
    }
}

/// A tinted type tile; the glyph differs per type so color is never the only signal.
#[component]
pub fn FileTile(
    #[prop(into)] kind: String,
    #[prop(into)] name: String,
    #[prop(into)] media_type: String,
    #[prop(optional)] large: bool,
) -> impl IntoView {
    let ty = file_type(&kind, &name, &media_type);
    view! { <span class=format!("file-tile file-tile-{ty}{}", if large { " is-large" } else { "" }) aria-hidden="true"><Icon name=icon_for(ty)/></span> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_follow_the_react_rules() {
        assert_eq!(file_type("folder", "x.png", ""), "folder");
        assert_eq!(file_type("file", "a.MD", "application/octet-stream"), "doc");
        assert_eq!(
            file_type_label("file", "notes.md", "text/markdown"),
            "Markdown"
        );
        assert_eq!(file_type("file", "data.bin", "application/json"), "code");
        assert_eq!(file_type("file", "x", "text/plain"), "doc");
        assert_eq!(file_type("file", "archive.tar.gz", ""), "archive");
    }
}
