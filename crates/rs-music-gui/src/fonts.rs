use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
};

#[derive(Clone)]
struct Face {
    path: PathBuf,
    index: u32,
}

#[derive(Default)]
pub(crate) struct SystemFonts {
    families: BTreeMap<String, Face>,
    chinese: Vec<Face>,
    default_face: Option<Face>,
}

impl SystemFonts {
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.families.keys().map(String::as_str)
    }

    pub(crate) fn install(&self, ctx: &egui::Context, selected: Option<&str>) {
        let mut fonts = FontDefinitions::default();
        let proportional = FontFamily::Proportional;
        let monospace = FontFamily::Monospace;
        let primary = selected
            .and_then(|name| {
                let chosen = self
                    .families
                    .get(name)
                    .and_then(|face| Some((face, load(face)?)));
                if chosen.is_none() {
                    tracing::warn!(family = name, "Selected system font is unavailable");
                }
                chosen
            })
            .or_else(|| {
                let face = self.default_face.as_ref()?;
                Some((face, load(face)?))
            });
        let primary_face = primary.map(|(face, data)| {
            let key = "system-primary".to_owned();
            fonts.font_data.insert(key.clone(), data);
            fonts
                .families
                .entry(proportional.clone())
                .or_default()
                .insert(0, key);
            face
        });
        let mut fallback_files = HashSet::new();
        let mut loaded = 0;
        for face in &self.chinese {
            if !fallback_files.insert(&face.path) {
                continue;
            }
            if primary_face
                .is_some_and(|primary| primary.path == face.path && primary.index == face.index)
            {
                fonts
                    .families
                    .entry(monospace.clone())
                    .or_default()
                    .push("system-primary".into());
                loaded += 1;
                continue;
            }
            if let Some(data) = load(face) {
                let key = format!("system-fallback-{loaded}");
                fonts.font_data.insert(key.clone(), data);
                for family in [&proportional, &monospace] {
                    fonts
                        .families
                        .entry(family.clone())
                        .or_default()
                        .push(key.clone());
                }
                loaded += 1;
            }
        }
        if loaded == 0 {
            tracing::warn!("No usable system Chinese font was found");
        }
        ctx.set_fonts(fonts);
    }
}

fn load(face: &Face) -> Option<Arc<FontData>> {
    let bytes = match fs::read(&face.path) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(path = %face.path.display(), %error, "Could not read system font");
            return None;
        }
    };
    if ab_glyph::FontRef::try_from_slice_and_index(&bytes, face.index).is_err() {
        tracing::warn!(path = %face.path.display(), index = face.index, "Could not parse system font face");
        return None;
    }
    let mut data = FontData::from_owned(bytes);
    data.index = face.index;
    Some(Arc::new(data))
}

#[cfg(all(unix, not(target_os = "macos")))]
impl SystemFonts {
    pub(crate) fn discover() -> Self {
        let mut fonts = Self::default();
        fonts.default_face = query(
            "fc-match",
            &[
                "-f",
                "%{file}\t%{index}",
                "sans-serif:outline=true:scalable=true",
            ],
        )
        .and_then(|line| {
            let (path, index) = line.split_once('\t')?;
            face(path, index)
        });
        let mut regular = HashSet::new();
        if let Some(list) = query(
            "fc-list",
            &[
                "-f",
                "%{family[0]}\t%{style[0]}\t%{file}\t%{index}\n",
                ":outline=true:scalable=true",
            ],
        ) {
            for line in list.lines() {
                let parts: Vec<_> = line.split('\t').collect();
                let [family, style, path, index] = parts.as_slice() else {
                    continue;
                };
                let Some(face) = face(path, index) else {
                    continue;
                };
                if matches!(*style, "Regular" | "Book" | "Roman" | "Normal")
                    && regular.insert(*family)
                {
                    fonts.families.insert((*family).into(), face);
                } else {
                    fonts.families.entry((*family).into()).or_insert(face);
                }
            }
        }
        if let Some(list) = query(
            "fc-match",
            &[
                "-s",
                "-f",
                "%{file}\t%{index}\t%{charset}\n",
                "sans-serif:lang=zh-cn:charset=4e2d:outline=true:scalable=true",
            ],
        ) {
            for line in list.lines() {
                let parts: Vec<_> = line.split('\t').collect();
                if let [path, index, charset] = parts.as_slice()
                    && contains_han(charset)
                    && let Some(face) = face(path, index)
                {
                    fonts.chinese.push(face);
                }
            }
        }
        fonts
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn face(path: &str, index: &str) -> Option<Face> {
    Some(Face {
        path: PathBuf::from(path),
        index: index.parse::<u32>().ok()? & 0xffff,
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn query(command: &str, args: &[&str]) -> Option<String> {
    match std::process::Command::new(command).args(args).output() {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => {
            tracing::warn!(command, status = %output.status, "Could not enumerate system fonts");
            None
        }
        Err(error) => {
            tracing::warn!(command, %error, "Could not enumerate system fonts");
            None
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn contains_han(charset: &str) -> bool {
    charset.split_ascii_whitespace().any(|range| {
        let (start, end) = range.split_once('-').unwrap_or((range, range));
        let (Ok(start), Ok(end)) = (u32::from_str_radix(start, 16), u32::from_str_radix(end, 16))
        else {
            return false;
        };
        [
            (0x3400, 0x4dbf),
            (0x4e00, 0x9fff),
            (0xf900, 0xfaff),
            (0x20000, 0x323af),
        ]
        .into_iter()
        .any(|(first, last)| start <= last && end >= first)
    })
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
impl SystemFonts {
    pub(crate) fn discover() -> Self {
        let mut fonts = Self::default();
        #[cfg(target_os = "windows")]
        let candidates = {
            let dir = std::env::var_os("WINDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
                .join("Fonts");
            [
                ("Microsoft YaHei", dir.join("msyh.ttc")),
                ("SimSun", dir.join("simsun.ttc")),
                ("SimSun ExtB", dir.join("simsunb.ttf")),
            ]
        };
        #[cfg(target_os = "macos")]
        let candidates = [
            (
                "PingFang",
                PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
            ),
            (
                "Songti",
                PathBuf::from("/System/Library/Fonts/Supplemental/Songti.ttc"),
            ),
        ];
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let candidates: [(&str, PathBuf); 0] = [];
        for (name, path) in candidates {
            if path.is_file() {
                let face = Face { path, index: 0 };
                fonts.chinese.push(face.clone());
                fonts.families.insert(name.into(), face);
            }
        }
        fonts.default_face = fonts.chinese.first().cloned();
        fonts
    }
}
