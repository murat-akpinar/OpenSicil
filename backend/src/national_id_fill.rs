// --- START FEATURE: national-id-fill ---
// TC kimlik no'nun AD'den toplu dolumu (ADR-112 madde 5, ADR-106 madde 5).
//
// Deger mutabakat taramasinda `ad_national_id_attribute` doluyken okunur ve bulguya
// sifreli yazilir; ayar bossa tarama hic okumaz, dolum da bos gecer. Diger bos
// alanlar gece worker'da kendiliginden dolar (`reconcile::fill_linked_identities`),
// bu alan dolamaz: kimlige yazmak AEAD + blind index ister (ADR-010) ve worker blind
// index uretmez. Bu yuzden dolum operatorun bastigi toplu eylemdir ve backend'dedir.
//
// Yalnizca bos alan dolar; TR kontrol hanelerinden gecmeyen ve baska kimlikte duran
// numara atlanir (tekillik `NOT EXISTS` + benzersiz indeks). Denetime deger girmez.
use sqlx::PgPool;

use crate::national_id::{self, Keys};

/// Hedefin bagli ve numarasi bos kimlikleri, bulgudaki sifreli degerle; sayi ve
/// liste ayni kosuldan okunur.
macro_rules! candidates_from {
    () => {
        "FROM reconcile_findings f JOIN identities i ON i.id = f.identity_id \
         WHERE f.target_system_id = $1 AND f.national_id_enc IS NOT NULL \
           AND i.national_id_enc IS NULL AND i.deleted_at IS NULL"
    };
}
const CANDIDATES_SQL: &str = concat!(
    "SELECT f.identity_id, f.national_id_enc ",
    candidates_from!(),
    " ORDER BY f.identity_id"
);
const PENDING_SQL: &str = concat!("SELECT count(*) ", candidates_from!());

const WRITE_SQL: &str = "UPDATE identities SET national_id_enc = $2, national_id_bidx = $3, \
    national_id_country = $4 WHERE id = $1 AND national_id_enc IS NULL \
    AND NOT EXISTS (SELECT 1 FROM identities WHERE national_id_bidx = $3)";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Filled {
    pub filled: Vec<i64>,
    /// TR kontrol hanelerinden gecmeyen ya da cozulemeyen deger
    pub invalid: usize,
    /// Numara baska bir kimlikte duruyor
    pub duplicate: usize,
}

/// Ekrandaki dugmenin sayisi: dolabilecek kimlik (gecersiz olanlar dahil).
pub async fn pending(pool: &PgPool, target: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(PENDING_SQL)
        .bind(target)
        .fetch_one(pool)
        .await
}

pub async fn fill(pool: &PgPool, keys: &Keys<'_>, target: i64) -> Result<Filled, sqlx::Error> {
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(CANDIDATES_SQL)
        .bind(target)
        .fetch_all(pool)
        .await?;
    let mut out = Filled::default();
    for (identity_id, enc) in rows {
        let Some(id) = national_id::decrypt(keys.aead, &enc)
            .ok()
            .and_then(|raw| national_id::parse("TR", &raw).ok())
        else {
            out.invalid += 1;
            continue;
        };
        let written = sqlx::query(WRITE_SQL)
            .bind(identity_id)
            .bind(national_id::encrypt(keys.aead, &id))
            .bind(national_id::blind_index(keys.blind_index, &id))
            .bind(&id.country)
            .execute(pool)
            .await;
        match written {
            Ok(done) if done.rows_affected() == 1 => out.filled.push(identity_id),
            Ok(_) => out.duplicate += 1,
            // Ayni anda yazilan ikinci kayit benzersiz indekse takilir: yine mukerrer
            Err(e)
                if e.as_database_error()
                    .is_some_and(|d| d.is_unique_violation()) =>
            {
                out.duplicate += 1
            }
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}
// --- END FEATURE: national-id-fill ---

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: Keys<'static> = Keys {
        aead: &[7; crate::crypto::KEY_LEN],
        blind_index: &[9; crate::crypto::KEY_LEN],
    };

    // ADR-112 madde 5: bos alan dolar, dolu alana dokunulmaz, gecersiz ve baska
    // kimlikte duran numara atlanir; ikinci kosu bir sey yapmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn fills_only_empty_valid_and_unique_national_ids() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [ayse, ali] = crate::test_support::seed_two_identities(&pool).await;
        let third: i64 = sqlx::query_scalar(
            "INSERT INTO identities (given_name, surname, department_id, primary_role_id, \
             employment_type, start_date) SELECT 'Veli', 'Can', department_id, primary_role_id, \
             'permanent', current_date FROM identities WHERE id = $1 RETURNING id",
        )
        .bind(ayse)
        .fetch_one(&pool)
        .await
        .unwrap();
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        let tr = |v: &str| national_id::parse("TR", v).unwrap();
        // Ayse'nin numarasi Veli'nin bulgusunda da duruyor: Veli mukerrer olur
        national_id::store(&pool, &KEYS, ayse, &tr("10000000146"))
            .await
            .unwrap();
        let enc = |v: &str| national_id::encrypt(KEYS.aead, &tr(v));
        let bad = national_id::encrypt(
            KEYS.aead,
            &national_id::NationalId {
                country: "TR".into(),
                value: "12345678901".into(),
            },
        );
        for (identity, value, name) in [
            (ayse, enc("19999999936"), "ayse"),
            (ali, enc("29999999904"), "ali"),
            (third, enc("10000000146"), "veli"),
        ] {
            sqlx::query(
                "INSERT INTO reconcile_findings (target_system_id, read_job_id, external_id, kind, \
                 identity_id, account_name, national_id_enc) VALUES ($1, $5, $2, 'observed', $3, $2, $4)",
            )
            .bind(target)
            .bind(name)
            .bind(identity)
            .bind(value)
            .bind(job)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert_eq!(
            pending(&pool, target).await.unwrap(),
            2,
            "dolu Ayşe sayılmaz"
        );

        let out = fill(&pool, &KEYS, target).await.unwrap();
        assert_eq!(
            out,
            Filled {
                filled: vec![ali],
                invalid: 0,
                duplicate: 1
            }
        );
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT national_id_enc FROM identities WHERE id = $1")
                .bind(ali)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            national_id::decrypt(KEYS.aead, &stored).unwrap(),
            "29999999904"
        );
        let ayse_now: Vec<u8> =
            sqlx::query_scalar("SELECT national_id_enc FROM identities WHERE id = $1")
                .bind(ayse)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            national_id::decrypt(KEYS.aead, &ayse_now).unwrap(),
            "10000000146",
            "dolu alan ezilmez"
        );

        // Gecersiz numara atlanir; ikinci kosuda Veli yine mukerrer, yazilan yok
        sqlx::query(
            "UPDATE reconcile_findings SET national_id_enc = $1 WHERE external_id = 'veli'",
        )
        .bind(bad)
        .execute(&pool)
        .await
        .unwrap();
        let again = fill(&pool, &KEYS, target).await.unwrap();
        assert_eq!(
            (again.filled.len(), again.invalid, again.duplicate),
            (0, 1, 0)
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // Uc: auditor dugmeyi gormez ve 403 alir; hr doldurur, denetime deger girmez.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn only_authority_fills_and_the_audit_row_carries_no_value() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [_, ali] = crate::test_support::seed_two_identities(&pool).await;
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let state = crate::web::test_state(pool.clone(), "https://localhost");
        let id = national_id::parse("TR", "10000000146").unwrap();
        sqlx::query(
            "WITH j AS (INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id) \
             INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, external_id, \
             account_name, identity_id, national_id_enc) SELECT $1, j.id, 'observed', 'g', 'ali', $2, $3 FROM j",
        )
        .bind(target)
        .bind(ali)
        .bind(national_id::encrypt(&state.aead_key, &id))
        .execute(&pool)
        .await
        .unwrap();
        let app = crate::web::routes().with_state(state);
        let send = |method: &'static str, authority: &'static str| {
            let (app, pool) = (app.clone(), pool.clone());
            async move {
                let operator = crate::operator_session::Operator {
                    subject: format!("sub-{authority}"),
                    username: authority.to_string(),
                    email: String::new(),
                    authorities: vec![authority.to_string()],
                    auth_source: crate::operator_session::AuthSource::Oidc,
                    lang: crate::i18n::DEFAULT,
                };
                let token = crate::operator_session::create_session(&pool, &operator)
                    .await
                    .unwrap();
                let uri = match method {
                    "POST" => format!("/targets/{target}/reconcile/national-ids"),
                    _ => format!("/targets/{target}/reconcile"),
                };
                let cookie = format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME);
                let r = app
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(uri)
                            .header(header::COOKIE, cookie)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = r.status();
                let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (status, String::from_utf8(bytes.to_vec()).unwrap())
            }
        };

        let (_, page) = send("GET", "auditor").await;
        assert!(
            !page.contains("/reconcile/national-ids"),
            "auditor düğmeyi görmez"
        );
        assert_eq!(send("POST", "auditor").await.0, StatusCode::FORBIDDEN);
        let (_, page) = send("GET", "hr").await;
        assert!(page.contains("/reconcile/national-ids"), "{page}");
        assert_eq!(send("POST", "hr").await.0, StatusCode::SEE_OTHER);
        assert_eq!(pending(&pool, target).await.unwrap(), 0);
        let detail: String = sqlx::query_scalar(
            "SELECT detail::text FROM audit_log WHERE event_type = $1 AND identity_id = $2",
        )
        .bind(crate::audit::IDENTITY_FIELD_TAKEN)
        .bind(ali)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            detail.contains("national_id") && detail.contains("filled"),
            "{detail}"
        );
        assert!(
            !detail.contains("10000000146"),
            "denetime değer girmez: {detail}"
        );

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
