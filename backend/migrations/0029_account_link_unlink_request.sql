-- ADR-122: dizinde bulunamayan hesabin baglantisini operator kaldirabilir.
-- Backend yalnizca ISTEGI yazar (bu kolon), kaldirmayi worker yapar ve once
-- dizine bakar: hesap gercekten yoksa baglanti silinir, varsa istek dusurulur ve
-- baglanti korunur. `account_links`i yalnizca worker yazar (ADR-015); backend'in
-- bu kolon uzerindeki UPDATE yetkisi migrate.rs SERVICE_GRANTS'ta.
ALTER TABLE account_links ADD COLUMN unlink_requested_at timestamptz;
