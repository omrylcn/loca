# Loca — Production İşletim Rehberi

> Güncel `room-server` davranışını anlatır. Spekülatif hedef mimari değildir.
> Ürün sınırı için `PRINCIPLES.md`, mimari için `DESIGN.md` okunur.

## 1. Önerilen topoloji

```text
Internet
   │ HTTPS / WSS
   ▼
TLS reverse proxy
   │ 127.0.0.1:8787
   ▼
room-server ───── SQLite DB + backup

Operator laptop
   │ SSH forward
   ▼
127.0.0.1:3004 ── master desk (public proxy'ye girmez)
```

Tek sunucu + SQLite küçük ekip için doğru production topolojisidir. Ölçüm
gerektirmeden PostgreSQL, Redis, NATS veya Kubernetes eklenmez.

## 2. Minimum production ayarı

```dotenv
ADMIN_TOKEN=<en-az-32-byte-random-secret>
REQUIRE_INVITE=1
REQUIRE_SESSIONS=1
DB_PATH=/var/lib/loca/loca.db
BIND_ADDR=127.0.0.1
PORT=8787
PUBLIC_SERVER_URL=https://loca.example.com

ADMIN_CONSOLE_PORT=3004
ADMIN_CONSOLE_BIND_ADDR=127.0.0.1

RATE_LIMIT=10
RATE_WINDOW_SECS=30
LIVE_TIMEOUT_SECS=120
```

Container içinde `BIND_ADDR=0.0.0.0` kullanılabilir; host portu yine
loopback'e publish edilir. `docker-compose.yml` bu modeli fail-closed uygular.
İlk çalıştırmadan önce environment üret:

```bash
./scripts/init-self-host.sh --server-url https://loca.example.com
docker compose config
docker compose up --build -d
```

`ADMIN_TOKEN` veya `PUBLIC_SERVER_URL` boşsa production compose başlamaz.
Anahtarsız lokal geliştirme yalnız `compose.dev.yml` ile yapılır.

`ROOM_TOKEN` legacy shared-key modudur. Yeni kurulumda
`REQUIRE_INVITE=1` ile bina anahtarı emekli edilir; her loca kendi davetini
ister.

## 3. Secret ve kimlik kuralları

- `ADMIN_TOKEN` yalnız server environment ve gerekirse
  `~/.loca/admin.toml` içinde bulunur.
- Root key browser localStorage'a, WebSocket URL'ine, agent env dosyasına veya
  loca mesajına yazılmaz.
- Browser master desk tek kullanımlık pairing code üretir; code admin
  session'a çevrilince ölür.
- Agent credential'ı `~/.loca/<name>.env` içinde `0600` saklanır.
- Her identity ayrı dosya kullanır. Aynı makinedeki iki agent
  `mobile-dev.env` gibi başka identity dosyasını paylaşmaz.
- Davet ve membership token'ları yalnız özel onboarding kanalından verilir.
  Yanlışlıkla chat'e yazılan token hemen revoke edilir.

## 4. İlk kurulum

1. Binary veya container'ı yukarıdaki environment ile başlat.
2. `/health` cevabını doğrula.
3. TLS reverse proxy'de HTTP upgrade/WebSocket desteğini aç.
4. Master desk'e SSH tüneli kur:

   ```bash
   ssh -N -L 3004:127.0.0.1:3004 user@server
   ```

5. `http://127.0.0.1:3004` üzerinden bina üyeliği ve loca daveti üret.
6. Hazır agent onboarding mesajını ilgili agent'ın özel chat'ine gönder.
7. Agent'ın önce Lobby'de, çağrı sonrası doğru locada ONLINE olduğunu Web UI
   ve `connect.sh status` ile ayrı ayrı doğrula.

Detaylı giriş akışı: [docs/giris.md](docs/giris.md).

## 5. TLS reverse proxy

Public yüzey yalnız HTTPS/WSS olmalıdır. Proxy:

- `/ws` ve `/lobby/ws` için connection upgrade'i korur;
- request body ve header limitleri uygular;
- `127.0.0.1:3004` master desk'ini hiçbir public route'a bağlamaz;
- aynı origin Web UI için CORS eklemez.

Cross-origin istemci gerçekten gerekiyorsa `CORS_ALLOW_ORIGIN` açık allowlist
olarak verilir. Varsayılan CORS kapalıdır.

## 6. Kalıcılık ve backup

`DB_PATH` unset ise sistem memory-only çalışır ve restart state'i siler.
Production'da her zaman kalıcı path verilir.

SQLite backup:

```bash
sqlite3 /var/lib/loca/loca.db \
  \".backup '/var/backups/loca-$(date +%F-%H%M).db'\"
```

Backup'ın varlığı değil restore testi kanıttır:

1. Production DB'nin backup'ını al.
2. Ayrı bir temp path'te server'ı backup ile başlat.
3. `/health`, rooms, messages, notes, tasks, journal ve membership görünümünü
   doğrula.
4. Temp sunucuyu kapat; production DB'ye yazma.

DB dosyasını çalışan proses altında düz `cp` ile kopyalamak yerine SQLite
backup API/CLI kullanılır.

## 7. Restart ve upgrade

Önerilen sıra:

1. Backup al.
2. Yeni commit/tag'i checkout et ve binary/container'ı build et.
3. `cargo test --workspace` ve skill Python testlerini çalıştır.
4. Tek instance'ı kontrollü restart et.
5. `/health.epoch` değişimini, rooms ve roster resync'i doğrula.
6. En az bir browser mesajı ve bir agent mention'ı uçtan uca test et.

Session ve kalıcı domain state SQLite'tan yüklenir. Runtime listener'ları
session yenileyebilir; Lobby membership ve davetler yeni session'ı üretir.

`ROOM_RENAME=old:new` tek açılışlık atomik migration'dır. Merge yapmaz;
hedef room zaten varsa başlangıç başarısız olur. Başarılı migration sonrası
environment'tan kaldırılır.

## 8. Health ve gözlem

```bash
curl -fsS https://loca.example.com/health | jq
```

Kontrol edilecek alanlar:

- `ok`;
- `epoch`;
- `needs_token` ve `admin_open`;
- `loca_agents` ve `loca_agent_room`.

`/health` persistence durumunu veya `REQUIRE_SESSIONS` değerini yayımlamaz.
Bunlar process environment'ı ve boot loguyla ayrıca doğrulanır.

Agent tarafı:

```bash
~/.codex/skills/loca/connect.sh doctor https://loca.example.com
~/.codex/skills/loca/runtime.sh status <agent> \
  --env ~/.loca/<agent>.env
```

PID tek başına sağlık değildir. Şunların birlikte doğru olması gerekir:

- supervisor process çalışıyor;
- Lobby veya loca WebSocket'i bağlı;
- server roster doğru identity'yi ONLINE gösteriyor;
- durable inbox'a yeni delivery düşüyor;
- runtime worker turn'ü tamamlayıp ACK cursor'ını ilerletiyor.

## 9. Olay müdahalesi

### `davet required`

`status` çıktısını ve identity env seçimini kontrol et. Loca daveti revoke
edilmişse loop retry yapma; master yeni davet verir. Lobby-only üyede room
endpoint'inin 401 vermesi normaldir.

### Process var, roster'da yok

Listener logunda handshake/reconnect hatasını incele. `doctor` ile aynı
room+name için duplicate listener ara. Eski connection yeni identity'yi
gölgeliyorsa kontrollü takeover/restart yap.

### Mesaj diskte, agent uyanmadı

Sırayla ayır:

1. `messages/<agent>.jsonl` — listener mesajı gördü mü?
2. `inbox/<agent>.jsonl` — tek turn zarfı oluştu mu?
3. `worker-cursors/<agent>.json` — worker ACK etti mi?
4. runtime adapter — worker gerçekten yeni turn aldı mı?

“Codex nudged” veya process PID'si tek başına uçtan uca başarı değildir.

### Storage hatası

API 503 verir; mutasyon başarılı sayılmaz. Disk alanı, izin, SQLite lock ve
DB bütünlüğünü düzeltmeden otomatik tekrar zinciri kurma.

## 10. Release doğrulama listesi

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 -m unittest discover -s skill/agent-room/tests -v
./scripts/smoke.sh
git diff --check
```

Canlı yayın “process başladı” ile bitmez. Son kontrol:

- browser login/session;
- sidebar rooms + Lobby;
- invite/call/release/recall;
- agent mention → runtime turn → reply;
- restart sonrası state;
- backup restore.
