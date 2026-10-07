// --- START FEATURE: operational-settings ---
// ADR-131: isletme ayarlari `operational_settings` tablosunda, Ayarlar ekranindan
// verilir. Anahtar adlari eski ortam degiskeni adlariyla ayni. Dogrulama, okuyan
// tarafin kendi ayristiricisidir: bozuk deger tabloya hic girmez (madde 5).

use std::collections::HashMap;

use sqlx::PgPool;

/// Ekranin bugun duzenledigi anahtarlar; ADR-131'in sonraki kutucuklari ekler.
pub const EDITABLE: [&str; 1] = [crate::change_set::THRESHOLD_KEY];

/// Kaydedilecek bicim; hata metni alanin yaninda gosterilir.
pub fn validate(key: &str, raw: &str) -> Result<String, String> {
    match key {
        crate::change_set::THRESHOLD_KEY => {
            crate::change_set::parse_threshold(raw).map(|n| n.to_string())
        }
        other => Err(format!("{other} ekrandan değiştirilemez")),
    }
}

/// Ekranin gordugu hal: kayitli (ya da reddedilen formdaki) degerler ve hatalar.
#[derive(Debug, Default, Clone)]
pub struct View {
    pub values: HashMap<String, String>,
    pub errors: HashMap<String, String>,
}

impl View {
    pub fn value(&self, key: &str) -> &str {
        self.values.get(key).map_or("", String::as_str)
    }

    pub fn error(&self, key: &str) -> &str {
        self.errors.get(key).map_or("", String::as_str)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub key: &'static str,
    pub before: String,
    pub after: String,
}

/// Saf: formu dogrular ve kayitli degerlerle karsilastirir. Tek hata bile
/// varsa hicbir degisiklik donmez; formdaki ham degerler ekrana geri gider.
pub fn plan(
    current: &HashMap<String, String>,
    form: &HashMap<String, String>,
) -> Result<Vec<Change>, View> {
    let mut changes = Vec::new();
    let mut view = View {
        values: current.clone(),
        errors: HashMap::new(),
    };
    for key in EDITABLE {
        let Some(raw) = form.get(key) else { continue };
        view.values.insert(key.to_string(), raw.clone());
        match validate(key, raw) {
            Ok(after) => {
                let before = current.get(key).cloned().unwrap_or_default();
                if before != after {
                    changes.push(Change { key, before, after });
                }
            }
            Err(e) => {
                view.errors.insert(key.to_string(), e);
            }
        }
    }
    if view.errors.is_empty() {
        Ok(changes)
    } else {
        Err(view)
    }
}

pub async fn save(pool: &PgPool, changes: &[Change], by: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for c in changes {
        sqlx::query(
            "UPDATE operational_settings SET value = $2, updated_at = now(), updated_by = $3 \
             WHERE key = $1",
        )
        .bind(c.key)
        .bind(&c.after)
        .bind(by)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn plan_lists_only_changed_valid_values() {
        let current = map(&[("CHANGE_SET_THRESHOLD", "10")]);
        assert_eq!(
            plan(&current, &map(&[("CHANGE_SET_THRESHOLD", " 3 ")])).unwrap(),
            vec![Change {
                key: "CHANGE_SET_THRESHOLD",
                before: "10".into(),
                after: "3".into()
            }]
        );
        assert!(plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "10")]))
            .unwrap()
            .is_empty());
        // 0 gecerli: her duzenleme onaya duser (ADR-031)
        assert_eq!(
            plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "0")])).unwrap()[0].after,
            "0"
        );
        // Ekranin duzenlemedigi anahtar formdan gelse de yazilmaz
        assert!(plan(&current, &map(&[("DRY_RUN", "false")]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn invalid_value_is_rejected_with_its_raw_text_kept() {
        let current = map(&[("CHANGE_SET_THRESHOLD", "10")]);
        let view = plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "on")])).unwrap_err();
        assert_eq!(view.value("CHANGE_SET_THRESHOLD"), "on");
        assert!(view.error("CHANGE_SET_THRESHOLD").contains("tam sayı"));
        assert!(plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "-1")])).is_err());
    }
}
// --- END FEATURE: operational-settings ---
