pub mod fetcher;
pub mod parser;

use serde::{Deserialize, Serialize};

/// Metadata about a downloaded filter list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListMeta {
    pub name: String,
    pub url: String,
    pub last_fetched: Option<u64>, // Unix timestamp
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// All known filter lists.
pub fn builtin_lists() -> Vec<ListMeta> {
    vec![
        ListMeta {
            name: "easylist".into(),
            url: "https://easylist.to/easylist/easylist.txt".into(),
            last_fetched: None,
            etag: None,
            last_modified: None,
        },
        ListMeta {
            name: "easyprivacy".into(),
            url: "https://easylist.to/easylist/easyprivacy.txt".into(),
            last_fetched: None,
            etag: None,
            last_modified: None,
        },
        ListMeta {
            name: "adguard-base".into(),
            url: "https://filters.adtidy.org/extension/ublock/filters/2.txt".into(),
            last_fetched: None,
            etag: None,
            last_modified: None,
        },
        ListMeta {
            name: "ublock-annoyances".into(),
            url: "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/annoyances.txt".into(),
            last_fetched: None,
            etag: None,
            last_modified: None,
        },
        ListMeta {
            name: "fanboy-annoyance".into(),
            url: "https://secure.fanboy.co.nz/fanboy-annoyance.txt".into(),
            last_fetched: None,
            etag: None,
            last_modified: None,
        },
        ListMeta {
            name: "adguard-cname".into(),
            url: "https://raw.githubusercontent.com/AdguardTeam/cname-trackers/master/data/combined_disguised_trackers.txt".into(),
            last_fetched: None,
            etag: None,
            last_modified: None,
        },
    ]
}
