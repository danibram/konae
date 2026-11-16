//! The size limits a person meets when an app travels to the installation,
//! and the sentences that name them. Each sentence names the size sent and
//! the limit. src/lib/size.ts is the editor's and the CLI's twin, and the
//! `sizes` and `sentences` of sources-cases.json are the cases both run.

/// The most bytes a Build Artifact holds once base64 is decoded.
pub const ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
/// The most bytes of JSON one release payload holds.
pub const RELEASE_BYTES: usize = 26_000_000;
/// The most bytes one owner upload holds when it names no component of an export.
pub const UPLOAD_BYTES: usize = 32 * 1024 * 1024;

/// `bytes` for a person: whole bytes under 1 KiB, else KiB or MiB with one
/// decimal, rounded up so a size over a limit never reads as the limit.
pub fn text(bytes: usize) -> String {
    let bytes = bytes as u64;
    if bytes == 1 {
        return "1 byte".to_owned();
    }
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let kib = (bytes * 10).div_ceil(1024);
    if kib < 10 * 1024 {
        return format!("{}.{} KiB", kib / 10, kib % 10);
    }
    let mib = (bytes * 10).div_ceil(1024 * 1024);
    format!("{}.{} MiB", mib / 10, mib % 10)
}

/// "The largest files are a (x), b (y) and c (z).": the three largest by
/// size, a tie in the byte order of their paths.
pub fn largest<'a>(files: impl IntoIterator<Item = (&'a str, usize)>) -> String {
    let mut sorted: Vec<(&str, usize)> = files.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let named: Vec<String> = sorted.iter().take(3).map(|(path, bytes)| format!("{path} ({})", text(*bytes))).collect();
    match named.as_slice() {
        [] => String::new(),
        [one] => format!("The largest file is {one}."),
        [first, second] => format!("The largest files are {first} and {second}."),
        [first, second, third] => format!("The largest files are {first}, {second} and {third}."),
        _ => unreachable!(),
    }
}

/// The refusal of a Build Artifact over [`ARTIFACT_BYTES`], from the decoded size of each file.
pub fn artifact_refused<'a>(files: impl IntoIterator<Item = (&'a str, usize)> + Clone) -> String {
    let total: usize = files.clone().into_iter().map(|(_, bytes)| bytes).sum();
    format!("dist/ holds {}. A Build Artifact holds at most 16 MiB. {}", text(total), largest(files))
}

/// The refusal of a release payload over [`RELEASE_BYTES`].
pub fn release_refused(bytes: usize) -> String {
    let tenths = (bytes as u64 * 10).div_ceil(1_000_000);
    format!("The release is {}.{} MB of JSON. A release holds at most 26 MB.", tenths / 10, tenths % 10)
}

/// The refusal of an owner upload that names no component, outside 1 byte to [`UPLOAD_BYTES`].
pub fn upload_refused(bytes: usize) -> String {
    format!("The upload is {}. An upload holds 1 byte to 32 MiB.", text(bytes))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    #[test]
    fn sizes_and_sentences_match_every_case_the_editor_runs() {
        let file: Value = serde_json::from_str(include_str!("sources-cases.json")).unwrap();
        for case in file["sizes"].as_array().unwrap() {
            assert_eq!(super::text(case["bytes"].as_u64().unwrap() as usize), case["text"], "{case}");
        }
        for case in file["sentences"].as_array().unwrap() {
            let sizes: Vec<(String, usize)> = case["files"].as_object().map(|files| files.iter().map(|(path, bytes)| (path.clone(), bytes.as_u64().unwrap() as usize)).collect()).unwrap_or_default();
            let bytes = case["bytes"].as_u64().unwrap_or(0) as usize;
            let sentence = match case["limit"].as_str().unwrap() {
                "artifact" => super::artifact_refused(sizes.iter().map(|(path, bytes)| (path.as_str(), *bytes))),
                "release" => super::release_refused(bytes),
                "upload" => super::upload_refused(bytes),
                other => panic!("no limit {other}"),
            };
            assert_eq!(sentence, case["sentence"], "{}", case["case"]);
        }
    }
}
