pub(crate) const CHAOS_ZONE_ID: &str = "chaos-zone";
pub(crate) const EXTREME_ZONE_ID: &str = "extreme";
pub(crate) const SERVER_ZONE_IDS: &[&str] = &[CHAOS_ZONE_ID, EXTREME_ZONE_ID];

pub(crate) const SERVER_MAP_FILENAME: &str = "bdhadar.lvl";
pub(crate) const SERVER_MAP_BYTES: &[u8] =
    include_bytes!("../../../../assets/zones/chaos-zone/bdhadar.lvl");
pub(crate) const SERVER_NEWS_FILENAME: &str = "news.txt";
pub(crate) const SERVER_NEWS_BYTES: &[u8] =
    include_bytes!("../../../../assets/zones/chaos-zone/news.txt");
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const SERVER_LVZ_FILES: &[(&str, &[u8])] = CHAOS_LVZ_FILES;

const EXTREME_MAP_FILENAME: &str = "doomsy8.lvl";
const EXTREME_MAP_BYTES: &[u8] = include_bytes!("../../../../assets/zones/extreme/doomsy8.lvl");
const EXTREME_NEWS_BYTES: &[u8] = b"Extreme\n";

pub(super) struct ZoneDefinition {
    pub(super) id: &'static str,
    pub(super) display_name: &'static str,
    pub(super) map_filename: &'static str,
    pub(super) map_bytes: &'static [u8],
    pub(super) news_filename: &'static str,
    pub(super) news_bytes: &'static [u8],
    pub(super) settings_bytes: &'static [u8],
    pub(super) lvz_files: &'static [(&'static str, &'static [u8])],
}

pub(super) const ZONE_DEFINITIONS: &[ZoneDefinition] = &[
    ZoneDefinition {
        id: CHAOS_ZONE_ID,
        display_name: "Chaos Zone",
        map_filename: SERVER_MAP_FILENAME,
        map_bytes: SERVER_MAP_BYTES,
        news_filename: SERVER_NEWS_FILENAME,
        news_bytes: SERVER_NEWS_BYTES,
        settings_bytes: include_bytes!("../../../../assets/zones/chaos-zone/arena.conf"),
        lvz_files: CHAOS_LVZ_FILES,
    },
    ZoneDefinition {
        id: EXTREME_ZONE_ID,
        display_name: "Extreme",
        map_filename: EXTREME_MAP_FILENAME,
        map_bytes: EXTREME_MAP_BYTES,
        news_filename: SERVER_NEWS_FILENAME,
        news_bytes: EXTREME_NEWS_BYTES,
        settings_bytes: include_bytes!("../../../../assets/zones/extreme/arena.conf"),
        lvz_files: EXTREME_LVZ_FILES,
    },
];

const CHAOS_LVZ_FILES: &[(&str, &[u8])] = &[
    (
        "aiq.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/aiq.lvz"),
    ),
    (
        "bonus.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/bonus.lvz"),
    ),
    (
        "cpwavs.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/cpwavs.lvz"),
    ),
    (
        "crw1a.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/crw1a.lvz"),
    ),
    (
        "ctrw1b.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/ctrw1b.lvz"),
    ),
    (
        "cts1a.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/cts1a.lvz"),
    ),
    (
        "czi.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/czi.lvz"),
    ),
    (
        "g.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/g.lvz"),
    ),
    (
        "newb2a.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/newb2a.lvz"),
    ),
    (
        "rnk.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/rnk.lvz"),
    ),
    (
        "teamwavs.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/teamwavs.lvz"),
    ),
    (
        "urkills.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/urkills.lvz"),
    ),
    (
        "zmoc.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/zmoc.lvz"),
    ),
    (
        "zmoct.lvz",
        include_bytes!("../../../../assets/zones/chaos-zone/zmoct.lvz"),
    ),
];

const EXTREME_LVZ_FILES: &[(&str, &[u8])] = &[
    (
        "Epl.lvz",
        include_bytes!("../../../../assets/zones/extreme/Epl.lvz"),
    ),
    (
        "akoth5.lvz",
        include_bytes!("../../../../assets/zones/extreme/akoth5.lvz"),
    ),
    (
        "dp7.lvz",
        include_bytes!("../../../../assets/zones/extreme/dp7.lvz"),
    ),
    (
        "eghelp.lvz",
        include_bytes!("../../../../assets/zones/extreme/eghelp.lvz"),
    ),
    (
        "powerup.lvz",
        include_bytes!("../../../../assets/zones/extreme/powerup.lvz"),
    ),
    (
        "pubmoney.lvz",
        include_bytes!("../../../../assets/zones/extreme/pubmoney.lvz"),
    ),
    (
        "tut_overlay.lvz",
        include_bytes!("../../../../assets/zones/extreme/tut_overlay.lvz"),
    ),
    (
        "tutsoren.lvz",
        include_bytes!("../../../../assets/zones/extreme/tutsoren.lvz"),
    ),
];
