-- ADR-126: POST artik sayfa basmaz, 303 ile GET sayfasina yonlendirir. Mesaj
-- yonlendirmede kaybolmasin diye operatorun oturum satirinda bekler: POST yazar,
-- bir sonraki GET okur ve ayni ifadede siler (bir kez gorunur). Dil tercihi de
-- burada duruyor (0014), tasiyici yeni bir tablo istemiyor.
ALTER TABLE operator_sessions
  ADD COLUMN flash_info TEXT,
  ADD COLUMN flash_error TEXT;
