#!/usr/bin/env bash
# N-14: docs/09 "compose → Kubernetes eşlemesi"nin tek node'lu kümede (kind) bir kez
# doğrulanması (ADR-061). k8s-lab/*.yaml uygulanır; sınananlar:
#   1. backend iki kopya, worker tek kopya (Recreate) ayağa kalkıyor;
#   2. migration Job'ı bitmeden servisler hazır olmuyor (önce servisler, sonra Job);
#   3. worker pod'u silinince yenisi kuyruğu sürdürüyor (N_JOBS kimlik işi, lab Samba'da hesap);
#   ek: oturum iki backend kopyasında da geçerli (ADR-061 madde 5), metrik ucu pod içinden,
#   worker canlılık yoklaması (worker-health) yeniden başlatmıyor.
# Gerekenler: kind kümesi (KIND_CONTEXT, varsayılan kind-k8rs), yerel opensicil-* imajları
# (`docker compose build`), lab Samba (seed.sh + samba-lab/tls). Keycloak gerekmez (yerel admin).
# Küme ağından host portuna erişim: kind ağının geçidi + cert SAN'ındaki `samba-ad` adı (hostAliases).
# Sonunda namespace ve lab AD'deki OU=K8s ağacı silinir.
set -euo pipefail
cd "$(dirname "$0")/.."

CTX=${KIND_CONTEXT:-kind-k8rs}; CLUSTER=${CTX#kind-}; NS=${K8S_NAMESPACE:-opensicil-lab}
N_JOBS=${N_JOBS:-200}; PF_PORT=${PF_PORT:-18443}
SAMBA=${SAMBA_CONTAINER:-opensicil-samba-ad-1}
SAM_LDB=/var/lib/samba/private/sam.ldb
K8S_OU="OU=K8s,DC=opensicil,DC=lab"
USER_OU="OU=Personel,$K8S_OU"; PASSIVE_OU="OU=Pasif,$USER_OU"; GROUP_OU="OU=Gruplar,$K8S_OU"
AD_LAB_PASSWORD=${AD_LAB_PASSWORD:-lab-only-not-secret-Aa1}
BASE="https://localhost:$PF_PORT"
WORK=$(mktemp -d); PF_PID=""; RESULTS=()
DBN=opensicil; OWNER=opensicil_owner; BU=opensicil_backend; WU=opensicil_worker

rand() { head -c 24 /dev/urandom | base64 | tr -d '/+=\n'; }
k() { kubectl --context "$CTX" -n "$NS" "$@"; }
samba() { docker exec "$SAMBA" "$@"; }
dbq() { k exec deploy/db -- psql -U "$OWNER" -d "$DBN" -v ON_ERROR_STOP=1 -qtAc "$1"; }
cookie_of() { grep -i "^set-cookie: $1=" "$2" | head -1 | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r'; }
epoch() { date -d "$1" +%s; }
record() { RESULTS+=("$1"); echo "  → $1"; }
cleanup() {
  [ -n "$PF_PID" ] && kill "$PF_PID" 2>/dev/null || true
  kubectl --context "$CTX" delete namespace "$NS" --wait=true --timeout=180s >/dev/null 2>&1 || true
  samba samba-tool ou delete "$K8S_OU" --force-subtree-delete >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT
trap 'echo "HATA (satır $LINENO): $BASH_COMMAND" >&2' ERR

echo "1) ön koşullar: küme, imajlar, lab Samba"
kubectl --context "$CTX" get nodes >/dev/null
IMAGES=(opensicil-backend:latest opensicil-worker:latest opensicil-nginx:latest)
for img in "${IMAGES[@]}"; do
  docker image inspect "$img" >/dev/null 2>&1 || { echo "imaj yok: $img (docker compose build)"; exit 1; }
done
GW=$(docker network inspect kind -f '{{range .IPAM.Config}}{{.Gateway}} {{end}}' | tr ' ' '\n' | grep -m1 '^[0-9]')
DNS=$(kubectl --context "$CTX" -n kube-system get svc kube-dns -o jsonpath='{.spec.clusterIP}')
[ -n "$GW" ] && [ -n "$DNS" ] || { echo "kind ağ geçidi ya da küme DNS'i bulunamadı"; exit 1; }
samba samba-tool ou delete "$K8S_OU" --force-subtree-delete >/dev/null 2>&1 || true
for ou in "$K8S_OU" "$USER_OU" "$PASSIVE_OU" "$GROUP_OU"; do samba samba-tool ou add "$ou" >/dev/null; done
T0=$(date +%s)
# postgres:18.6-alpine yüklenmez: çok platformlu imajı `kind load` içe aktaramıyor, node kendisi çeker
kind load docker-image "${IMAGES[@]}" --name "$CLUSTER" 2>&1 | grep -iv 'loading\|already present' || true
record "imajlar kümeye yüklendi ($(( $(date +%s) - T0 )) sn); host geçidi $GW, küme DNS $DNS"

echo "2) namespace, Secret'lar (servis başına, ADR-006) ve ConfigMap'ler"
kubectl --context "$CTX" delete namespace "$NS" --wait=true --timeout=180s >/dev/null 2>&1 || true
kubectl --context "$CTX" create namespace "$NS" >/dev/null
OWNER_PW=$(rand); BP=$(rand); WP=$(rand); MT=$(rand)
AEAD=$(head -c 32 /dev/urandom | base64); BIDX=$(head -c 32 /dev/urandom | base64)
k create secret generic db-owner --from-literal=user=$OWNER --from-literal=password="$OWNER_PW" --from-literal=db=$DBN >/dev/null
k create secret generic migrate-env --from-literal=DATABASE_URL="postgres://$OWNER:$OWNER_PW@db:5432/$DBN" \
  --from-literal=POSTGRES_BACKEND_USER=$BU --from-literal=POSTGRES_BACKEND_PASSWORD="$BP" \
  --from-literal=POSTGRES_WORKER_USER=$WU --from-literal=POSTGRES_WORKER_PASSWORD="$WP" >/dev/null
k create secret generic backend-env --from-literal=DATABASE_URL="postgres://$BU:$BP@db:5432/$DBN" \
  --from-literal=AEAD_MASTER_KEY="$AEAD" --from-literal=BLIND_INDEX_KEY="$BIDX" --from-literal=METRICS_TOKEN="$MT" >/dev/null
k create secret generic worker-env --from-literal=DATABASE_URL="postgres://$WU:$WP@db:5432/$DBN" \
  --from-literal=AEAD_MASTER_KEY="$AEAD" --from-literal=BLIND_INDEX_KEY="$BIDX" >/dev/null
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj '/CN=localhost' -addext 'subjectAltName=DNS:localhost' \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" >/dev/null 2>&1
k create secret generic nginx-tls --from-file=cert.pem="$WORK/cert.pem" --from-file=key.pem="$WORK/key.pem" >/dev/null
k create configmap common --from-literal=OWNERSHIP_MODE_ENABLED=false --from-literal=HOURLY_DESTRUCTIVE_LIMIT=50 \
  --from-literal=HOURLY_GRANT_LIMIT=500 --from-literal=HOURLY_FIRST_PASSWORD_LIMIT=50 --from-literal=EMERGENCY_QUOTA=5 \
  --from-literal=SENSITIVE_MAPPING_ENABLED=false --from-literal=TZ=Europe/Istanbul >/dev/null
k create configmap backend-config --from-literal=PUBLIC_URL="$BASE" \
  --from-literal=APPROVAL_TIMELOCK_HOURS=0 --from-literal=AD_CA_FILE=/etc/opensicil/ad-ca.pem >/dev/null
k create configmap worker-config --from-literal=DRY_RUN=false --from-literal=FIRST_LOGIN_CHANGE_REQUIRED=true \
  --from-literal=AD_CA_FILE=/etc/opensicil/ad-ca.pem --from-literal=USERNAME_TEMPLATE= --from-literal=EMAIL_LOCAL_TEMPLATE= \
  --from-literal=AD_MANAGED_USER_OUS="$USER_OU" --from-literal=AD_PASSIVE_OU="$PASSIVE_OU" \
  --from-literal=AD_MANAGED_GROUP_OUS="$GROUP_OU" --from-literal=ZIMBRA_MANAGED_DOMAINS= >/dev/null
k create configmap ad-ca --from-file=ad-ca.pem=samba-lab/tls/ca.pem >/dev/null
# nginx.conf'taki resolver Docker'ın gömülü DNS'i; kümede kube-dns. nginx resolver'ı
# arama alanı (search) uygulamaz: upstream adı FQDN olmalı (backend.<ns>.svc.cluster.local)
sed "s/resolver 127\.0\.0\.11/resolver $DNS/" nginx/nginx.conf >"$WORK/nginx.conf"
sed "s#http://backend:8000#http://backend.$NS.svc.cluster.local:8000#" nginx/proxy.conf >"$WORK/proxy.conf"
k create configmap nginx-conf --from-file=nginx.conf="$WORK/nginx.conf" --from-file=proxy.conf="$WORK/proxy.conf" >/dev/null

echo "3) önce servisler (Job yok): şema yokken hazır olmamalılar"
sed "s/__HOST_GATEWAY__/$GW/" k8s-lab/opensicil.yaml | k apply -f - >/dev/null
k rollout status deploy/db --timeout=180s >/dev/null
sleep 45
READY=$(k get deploy backend -o jsonpath='{.status.readyReplicas}'); READY=${READY:-0}
RESTARTS=$(k get pods -l app=backend -o jsonpath='{range .items[*]}{.status.containerStatuses[0].restartCount}{" "}{end}')
WREADY=$(k get deploy worker -o jsonpath='{.status.readyReplicas}'); WREADY=${WREADY:-0}
MSG=$(k logs -l app=backend --tail=1 2>&1 | head -1 | cut -c1-120 || true)
[ "$READY" = 0 ] && [ "$WREADY" = 0 ] || { echo "şema yokken servis hazır oldu: backend $READY, worker $WREADY"; exit 1; }
record "şema yokken 45 sn sonra: backend hazır 0/2 (yeniden başlatma: $RESTARTS), worker hazır 0/1; backend: \"$MSG\""

echo "4) migration Job'ı → servisler kendiliğinden hazır"
k apply -f k8s-lab/migrate-job.yaml >/dev/null
k wait --for=condition=complete job/migrate --timeout=300s >/dev/null
JOB_DONE=$(k get job migrate -o jsonpath='{.status.completionTime}')
T0=$(date +%s)
k rollout status deploy/backend --timeout=600s >/dev/null
k rollout status deploy/worker --timeout=600s >/dev/null
k rollout status deploy/nginx --timeout=300s >/dev/null
T_READY=$(( $(date +%s) - T0 ))
FIRST_READY=$(k get pods -l app=backend -o jsonpath='{range .items[*]}{.status.conditions[?(@.type=="Ready")].lastTransitionTime}{"\n"}{end}' | sort | head -1)
[ "$(epoch "$FIRST_READY")" -ge "$(epoch "$JOB_DONE")" ] || { echo "backend Job'dan önce hazır görünüyor: $FIRST_READY < $JOB_DONE"; exit 1; }
record "Job $JOB_DONE'de bitti; backend 2/2, worker 1/1, nginx 1/1 hazır (Job'dan $T_READY sn sonra, geri çekilme dahil); ilk backend Ready $FIRST_READY ≥ Job bitişi"

echo "5) nginx üzerinden giriş, Yapılandırma (lab AD), oturum iki kopyada"
k port-forward svc/nginx "$PF_PORT:8443" >/dev/null 2>&1 &
PF_PID=$!
for _ in $(seq 1 30); do [ "$(curl -sk -o /dev/null -w '%{http_code}' "$BASE/login")" = 200 ] && break; sleep 1; done
curl -sk -D "$WORK/h" -o "$WORK/login.html" -d 'username=admin&password=admin' "$BASE/login"
ADMIN=$(cookie_of opensicil_operator_session "$WORK/h" || true)
[ -n "$ADMIN" ] || { echo "yerel giriş başarısız:"; head -1 "$WORK/h"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/login.html" | head -3; k logs deploy/nginx --tail=3; exit 1; }
curl -sk -o /dev/null -b "$ADMIN" -d 'new_password=k8s-bootstrap-parola-1&confirm_password=k8s-bootstrap-parola-1' "$BASE/change-password"
curl -sk -o /dev/null -b "$ADMIN" \
  --data-urlencode "ad_host=samba-ad:6360" \
  --data-urlencode "ad_bind_dn=CN=Administrator,CN=Users,DC=opensicil,DC=lab" \
  --data-urlencode "ad_service_password=$AD_LAB_PASSWORD" \
  --data-urlencode "zimbra_url=" --data-urlencode "zimbra_admin_password=" \
  --data-urlencode "oidc_issuer=http://samba-ad:8081/realms/opensicil" \
  --data-urlencode "oidc_client_id=opensicil-backend" \
  --data-urlencode "oidc_client_secret=lab-only-not-secret" "$BASE/config"
OK=0; for _ in $(seq 1 10); do [ "$(curl -sk -o /dev/null -w '%{http_code}' -b "$ADMIN" "$BASE/")" = 200 ] && OK=$((OK + 1)); done
record "yerel admin oturumu nginx → Service → iki backend kopyası: 10 istekte $OK × 200 (oturum veritabanında, ADR-061 madde 5)"
AD=$(dbq "SELECT id FROM target_systems WHERE kind = 'ad'")
curl -sk -o /dev/null -b "$ADMIN" -X POST "$BASE/targets/$AD/catalog-refresh"
for _ in $(seq 1 60); do
  [ "$(dbq "SELECT count(*) FROM catalog_items WHERE kind = 'ou' AND display_name = 'Personel'")" = 1 ] && break; sleep 2
done
[ "$(dbq "SELECT count(*) FROM catalog_items WHERE kind = 'ou' AND display_name = 'Personel'")" = 1 ] || { echo "katalog gelmedi"; k logs -l app=worker --tail=20; exit 1; }

echo "6) $N_JOBS kimlik işi; sürerken worker pod'u silinir → yenisi sürdürür"
dbq "INSERT INTO roles (kind, name, slug) VALUES ('base', 'Herkes', 'herkes'), ('primary', 'Pod Uzmani', 'pod-uzmani');
INSERT INTO departments (name, slug) VALUES ('Kume', 'kume');
UPDATE target_systems SET default_container_item_id = (SELECT id FROM catalog_items WHERE kind = 'ou' AND display_name = 'Personel' LIMIT 1) WHERE id = $AD;
INSERT INTO identities (given_name, surname, employee_number, department_id, primary_role_id, employment_type, start_date)
  SELECT 'Kube', 'Pod' || i, 'K8S-' || i, (SELECT id FROM departments LIMIT 1),
         (SELECT id FROM roles WHERE kind = 'primary' AND NOT placeholder LIMIT 1), 'permanent', current_date - 1
  FROM generate_series(1, $N_JOBS) i;
INSERT INTO jobs (identity_id, target_system_id, priority) SELECT id, $AD, 1 FROM identities;" >/dev/null
OLD_POD=$(k get pods -l app=worker -o jsonpath='{.items[0].metadata.name}')
for _ in $(seq 1 120); do [ "$(dbq "SELECT count(*) FROM jobs WHERE status = 'succeeded'")" -ge $((N_JOBS / 10)) ] && break; sleep 1; done
DONE_BEFORE=$(dbq "SELECT count(*) FROM jobs WHERE status = 'succeeded'")
[ "$DONE_BEFORE" -ge $((N_JOBS / 10)) ] || { echo "işler ilerlemedi: $DONE_BEFORE"; k logs "$OLD_POD" --tail=20; exit 1; }
k logs -f "$OLD_POD" >"$WORK/old-worker.log" 2>/dev/null &
T0=$(date +%s)
k delete pod "$OLD_POD" --wait=false >/dev/null
NEW_POD=""
for _ in $(seq 1 120); do
  NEW_POD=$(k get pods -l app=worker -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.status.phase}{" "}{.status.conditions[?(@.type=="Ready")].status}{"\n"}{end}' | awk -v old="$OLD_POD" '$1 != old && $2 == "Running" && $3 == "True" {print $1}')
  [ -n "$NEW_POD" ] && break; sleep 1
done
T_SWITCH=$(( $(date +%s) - T0 ))
[ -n "$NEW_POD" ] || { echo "yeni worker pod'u hazır olmadı"; k get pods -l app=worker; exit 1; }
SIGTERM_LINE=$(grep -m1 'SIGTERM' "$WORK/old-worker.log" || echo "(SIGTERM satırı log'da yok)")
for _ in $(seq 1 600); do [ "$(dbq "SELECT count(*) FROM jobs WHERE status <> 'succeeded'")" = 0 ] && break; sleep 1; done
SUCCEEDED=$(dbq "SELECT count(*) FROM jobs WHERE status = 'succeeded'")
INTERVENTION=$(dbq "SELECT count(*) FROM jobs WHERE status = 'needs_intervention'")
HEARTBEAT=$(dbq "SELECT worker_id FROM worker_status" | cut -c1-40)
ACCOUNTS=$(samba ldbsearch -H $SAM_LDB -b "$USER_OU" -s one '(objectClass=user)' dn 2>/dev/null | grep -c '^dn:' || true)
record "worker $OLD_POD silindi ($DONE_BEFORE iş bitmişken): \"$SIGTERM_LINE\"; yeni pod $NEW_POD $T_SWITCH sn'de hazır; sonuç $SUCCEEDED/$N_JOBS iş, müdahale $INTERVENTION, AD'de $ACCOUNTS hesap, nabız satırı: $HEARTBEAT"

echo "7) metrik ucu (pod içinden, Bearer) ve worker canlılık yoklaması"
METRICS=$(k exec deploy/backend -- wget -qO- --header "Authorization: Bearer $MT" http://localhost:8000/metrics 2>/dev/null | grep -c '^opensicil_' || true)
NOAUTH=$(k exec deploy/backend -- sh -c 'wget -S -qO- http://localhost:8000/metrics 2>&1 || true' | grep -o 'HTTP/1.1 401' | head -1 || true)
sleep 30
WRESTARTS=$(k get pod "$NEW_POD" -o jsonpath='{.status.containerStatuses[0].restartCount}')
record "metrik ucu: $METRICS satır (tokensiz: ${NOAUTH:-401 değil}); yeni worker pod'u 30 sn sonra yeniden başlatma $WRESTARTS (worker-health canlılık)"

echo
echo "N-14 SONUÇ (küme $CTX, namespace $NS)"
printf '  %s\n' "${RESULTS[@]}"
[ "$SUCCEEDED" = "$N_JOBS" ] && [ "$INTERVENTION" = 0 ] && [ "$ACCOUNTS" = "$N_JOBS" ] && [ "$OK" = 10 ] && [ "$WRESTARTS" = 0 ] \
  && echo "N-14 TAMAM" || { echo "N-14 BAŞARISIZ"; exit 1; }
