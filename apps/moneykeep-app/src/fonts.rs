//! Installed fonts for Settings → Display, found with `fontdb` in the system font
//! folders. Only regular, upright faces are offered (one per family); files are read
//! when a font is chosen or previewed, never bundled or copied.

use std::collections::BTreeMap;

use mk_ui::{FontBytes, FontInfo, FontProvider};

pub struct SystemFonts {
    db: fontdb::Database,
    /// Family → the face to load.
    faces: BTreeMap<String, (fontdb::ID, bool)>,
}

impl SystemFonts {
    pub fn discover() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut faces: BTreeMap<String, (fontdb::ID, bool)> = BTreeMap::new();
        for face in db.faces() {
            if face.style != fontdb::Style::Normal || face.weight != fontdb::Weight::NORMAL {
                continue;
            }
            let Some((family, _)) = face.families.first() else {
                continue;
            };
            faces
                .entry(family.clone())
                .or_insert((face.id, face.monospaced));
        }
        Self { db, faces }
    }
}

impl FontProvider for SystemFonts {
    fn families(&self) -> Vec<FontInfo> {
        self.faces
            .iter()
            .map(|(family, (_, mono))| FontInfo {
                family: family.clone(),
                monospace: *mono,
            })
            .collect()
    }

    fn load(&self, family: &str) -> Option<FontBytes> {
        let (id, _) = self.faces.get(family)?;
        self.db.with_face_data(*id, |data, index| FontBytes {
            data: data.to_vec(),
            index,
        })
    }
}
