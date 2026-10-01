-- Elle kullanici adi ve cakisma mudahalesi (ADR-022, ADR-042, ADR-081).
-- requested_username: operatorun istedigi ad; worker ADR-011 normallestirmesiyle
-- dogrular, cakisirsa n eklemez (mudahale). name_conflict_override: "farkli kisi,
-- siradaki adi ver" karari; bagli olmayan hesap / kullanilmis ad cakismasinda
-- worker n + 1'e gecer. Ikisi de yalnizca username bosken anlamlidir; backend yazar.
ALTER TABLE identities
    ADD COLUMN requested_username TEXT,
    ADD COLUMN name_conflict_override BOOLEAN NOT NULL DEFAULT FALSE;
