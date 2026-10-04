use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

// The folder the user drops language folders into, under the app data
// directory. Same layout as the Euro-Office/dictionaries repo, so a language
// folder can be copied across without renaming anything.
pub const DIR_NAME: &str = "dictionaries";

#[derive(Serialize)]
pub struct UserDictionaries {
    pub valid: Vec<String>,
    pub skipped: Vec<String>,
}

// A dictionary folder name as the dictionaries repo spells it: a locale like
// uk_UA or es_ES. This whitelist is the security boundary of the handler
// branch that reads the user's disk, so it is stated as what IS allowed:
// every character a traversal needs (the dot of "..", both separators, the
// colon of "C:") is simply absent from it, and no normalization pass has to be
// trusted to have caught every spelling of the same escape.
pub fn is_folder_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// <stem>.aff or <stem>.dic, the two files hunspell loads. Nothing else in the
// folder is served: the worker never asks for the licenses or the README, and
// a narrower door is a smaller one to guard.
pub fn is_file_name(name: &str) -> bool {
    let mut parts = name.rsplitn(2, '.');
    let ext = match parts.next() {
        Some(ext) => ext,
        None => return false,
    };
    let stem = match parts.next() {
        Some(stem) => stem,
        None => return false,
    };
    (ext == "aff" || ext == "dic") && is_folder_name(stem)
}

// "dictionaries/uk_UA/uk_UA.aff" -> ("uk_UA", "uk_UA.aff"). Anything that is
// not exactly <folder>/<file> under the prefix, with both halves on the
// whitelist, is refused here and answered with a 404. The caller passes the
// already percent-decoded path, so an escape spelled %2e%2e arrives as ".."
// and is rejected like any other.
pub fn split_request(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("dictionaries/")?;
    let mut parts = rest.split('/');
    let folder = parts.next()?;
    let file = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !is_folder_name(folder) || !is_file_name(file) {
        return None;
    }
    Some((folder.to_string(), file.to_string()))
}

// A folder counts only when it carries the pair hunspell needs. Half a
// dictionary is worse than none: the worker's loader waits for both files
// before it reports a language ready, so an incomplete folder would leave its
// queue stalled, and a language that loaded nothing answers every word wrong
// rather than not at all. Both failure modes are avoided by never offering
// such a folder to the bridge in the first place.
pub fn classify(folders: &[(String, Vec<String>)]) -> (Vec<String>, Vec<String>) {
    let mut valid = Vec::new();
    let mut skipped = Vec::new();
    for (name, files) in folders {
        if !is_folder_name(name) {
            skipped.push(name.clone());
            continue;
        }
        let aff = format!("{}.aff", name);
        let dic = format!("{}.dic", name);
        if files.iter().any(|f| f == &aff) && files.iter().any(|f| f == &dic) {
            valid.push(name.clone());
        } else {
            skipped.push(name.clone());
        }
    }
    valid.sort();
    skipped.sort();
    (valid, skipped)
}

// Tauri's asset resolver does not answer "no such asset": when a path is
// missing it falls back to <path>.html, then to <path>/index.html, and finally
// to index.html itself, which is how a single page app keeps its routes
// working. A dictionary request has no business taking that road, and letting
// it would be the worst kind of wrong: the worker would receive 200 with the
// start screen's markup in it, hand that to hunspell as a dictionary and start
// underlining correct words instead of simply checking nothing. The bridge
// already never asks for a language it has not seen, so this is the second
// lock on the same door.
//
// The test is on the bytes and not on the mime type, which was the first
// attempt and was wrong in the worst direction: MimeType::parse_from_uri falls
// back to text/html for EVERY extension it does not know, and it knows neither
// .aff nor .dic, so a perfectly good bundled dictionary is announced as HTML
// exactly like the fallback page. Judging by mime therefore rejected en_US and
// es_ES too, the worker sat waiting on a 404 that never resolves, and spell
// check died for the languages that ship with the app. The content tells the
// two apart with no ambiguity: the fallback is an HTML document, an .aff opens
// with a comment or a directive and a .dic with its word count.
pub fn is_dictionary_asset(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace() && !matches!(b, 0xEF | 0xBB | 0xBF))
        .unwrap_or(bytes.len());
    let head: Vec<u8> = bytes[start..]
        .iter()
        .take(9)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    if head.is_empty() {
        return false;
    }
    !(head.starts_with(b"<!doctype") || head.starts_with(b"<html"))
}

pub fn user_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|dir| dir.join(DIR_NAME))
}

pub fn read_user_file(app: &AppHandle, folder: &str, file: &str) -> Option<Vec<u8>> {
    std::fs::read(user_dir(app)?.join(folder).join(file)).ok()
}

// A missing directory is the normal case, not an error: almost nobody has one.
fn scan(dir: &Path) -> Vec<(String, Vec<String>)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        // An empty file counts as absent. A zero byte .aff or .dic is not a
        // dictionary the worker can refuse: it loads, hunspell recognises
        // nothing, and every word in the document comes back wrong. Reporting
        // the folder as skipped is the only answer that cannot underline
        // correct text.
        let mut files = Vec::new();
        if let Ok(inner) = std::fs::read_dir(entry.path()) {
            for file in inner.flatten() {
                let has_bytes = file.metadata().map(|meta| meta.len() > 0).unwrap_or(false);
                if !has_bytes {
                    continue;
                }
                if let Ok(file_name) = file.file_name().into_string() {
                    files.push(file_name);
                }
            }
        }
        folders.push((name, files));
    }
    folders
}

// The bridge logs both lists, so a folder that was ignored says so in
// js-debug.log instead of failing silently.
#[tauri::command]
pub async fn list_user_dictionaries(app: AppHandle) -> UserDictionaries {
    let dir = match user_dir(&app) {
        Some(dir) => dir,
        None => {
            return UserDictionaries {
                valid: Vec::new(),
                skipped: Vec::new(),
            }
        }
    };
    let (valid, skipped) = classify(&scan(&dir));
    UserDictionaries { valid, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, files: &[&str]) -> (String, Vec<String>) {
        (
            name.to_string(),
            files.iter().map(|f| f.to_string()).collect(),
        )
    }

    #[test]
    fn accepts_the_locale_names_the_repo_uses() {
        assert!(is_folder_name("uk_UA"));
        assert!(is_folder_name("es_ES"));
        assert!(is_folder_name("ca_ES_valencia"));
        assert!(is_folder_name("az_Latn_AZ"));
        assert!(is_folder_name("en-GB"));
    }

    #[test]
    fn rejects_traversal_in_the_folder_name() {
        assert!(!is_folder_name(".."));
        assert!(!is_folder_name("."));
        assert!(!is_folder_name("../etc"));
        assert!(!is_folder_name("..\\windows"));
        assert!(!is_folder_name("/etc"));
        assert!(!is_folder_name("C:"));
        assert!(!is_folder_name("uk_UA/.."));
        assert!(!is_folder_name(""));
        assert!(!is_folder_name("a".repeat(65).as_str()));
    }

    #[test]
    fn accepts_only_the_two_hunspell_files() {
        assert!(is_file_name("uk_UA.aff"));
        assert!(is_file_name("uk_UA.dic"));
        assert!(is_file_name("hyph_uk_UA.dic"));
        assert!(!is_file_name("license.txt"));
        assert!(!is_file_name("uk_UA"));
        assert!(!is_file_name(".aff"));
        assert!(!is_file_name("uk_UA.aff.exe"));
        assert!(!is_file_name("../uk_UA.aff"));
        assert!(!is_file_name("..\\uk_UA.dic"));
    }

    #[test]
    fn splits_a_well_formed_request() {
        assert_eq!(
            split_request("dictionaries/uk_UA/uk_UA.aff"),
            Some(("uk_UA".to_string(), "uk_UA.aff".to_string()))
        );
    }

    #[test]
    fn refuses_everything_that_is_not_folder_slash_file() {
        assert_eq!(split_request("dictionaries/uk_UA"), None);
        assert_eq!(split_request("dictionaries/uk_UA/nested/uk_UA.aff"), None);
        assert_eq!(split_request("dictionaries//uk_UA.aff"), None);
        assert_eq!(split_request("dictionaries/"), None);
        assert_eq!(split_request("docmedia/image1.png"), None);
        assert_eq!(split_request("dictionariesX/uk_UA/uk_UA.aff"), None);
    }

    // The percent-decoded spellings arrive here already decoded, so these are
    // the exact strings the handler sees for an attempted escape.
    #[test]
    fn refuses_traversal_through_the_request_path() {
        assert_eq!(split_request("dictionaries/../../etc/passwd"), None);
        assert_eq!(split_request("dictionaries/../recent-files.json"), None);
        // Two components and a real extension, so only the name whitelist
        // stands between this one and the parent directory.
        assert_eq!(split_request("dictionaries/../uk_UA.aff"), None);
        assert_eq!(split_request("dictionaries/uk_UA/../uk_UA.dic"), None);
        assert_eq!(
            split_request("dictionaries/uk_UA/../../../etc/passwd"),
            None
        );
        assert_eq!(split_request("dictionaries/..\\..\\secret.aff"), None);
        assert_eq!(split_request("dictionaries//etc/passwd"), None);
        assert_eq!(split_request("dictionaries/C:/Windows/win.ini"), None);
        assert_eq!(split_request("dictionaries/uk_UA/%2e%2e/x.aff"), None);
    }

    #[test]
    fn a_folder_is_valid_only_with_both_files() {
        let (valid, skipped) = classify(&[
            folder("uk_UA", &["uk_UA.aff", "uk_UA.dic", "license.txt"]),
            folder("de_DE", &["de_DE.aff"]),
            folder("fr_FR", &["fr_FR.dic"]),
            folder("empty", &[]),
        ]);
        assert_eq!(valid, vec!["uk_UA"]);
        assert_eq!(skipped, vec!["de_DE", "empty", "fr_FR"]);
    }

    #[test]
    fn a_folder_whose_files_carry_another_name_is_skipped() {
        let (valid, skipped) = classify(&[folder("ukrainian", &["uk_UA.aff", "uk_UA.dic"])]);
        assert!(valid.is_empty());
        assert_eq!(skipped, vec!["ukrainian"]);
    }

    #[test]
    fn an_unusable_folder_name_is_reported_not_served() {
        let (valid, skipped) = classify(&[folder("../evil", &["../evil.aff", "../evil.dic"])]);
        assert!(valid.is_empty());
        assert_eq!(skipped, vec!["../evil"]);
    }

    // The exact bytes the start screen answered with on Ubuntu when a missing
    // language was requested through the old, mime based test.
    #[test]
    fn the_single_page_fallback_is_not_a_dictionary() {
        assert!(!is_dictionary_asset(b"<!DOCTYPE html><html lang=\"es\">"));
        assert!(!is_dictionary_asset(b"<html><body>x</body></html>"));
        assert!(!is_dictionary_asset(b"<!doctype html>"));
        assert!(!is_dictionary_asset(b"\n  <!DOCTYPE html>"));
        assert!(!is_dictionary_asset(b"\xef\xbb\xbf<!DOCTYPE html>"));
        assert!(!is_dictionary_asset(b""));
    }

    // Real openings: en_US.aff starts with a comment, uk_UA.aff with a
    // directive, and every .dic with the number of words it holds.
    #[test]
    fn a_real_dictionary_is_kept() {
        assert!(is_dictionary_asset(b"# 2024-01-29 (Marco A.G.Pinto)"));
        assert!(is_dictionary_asset(b"SET UTF-8\nTRY esiaonrtlc"));
        assert!(is_dictionary_asset(b"336673\n+cs=word/j"));
        assert!(is_dictionary_asset(b"49435\nabandon"));
    }

    #[test]
    fn no_folders_means_two_empty_lists() {
        let (valid, skipped) = classify(&[]);
        assert!(valid.is_empty());
        assert!(skipped.is_empty());
    }
}
