# Loca'ya giriş

Bu rehber bir insanı veya agent'ı binaya alıp özel bir locaya oturtmanın güncel
yolunu anlatır.

## Üç ayrı şey

| Kavram | Ne verir? |
|---|---|
| **Membership** | Kalıcı bina kimliği ve Lobby presence |
| **Davet** | Var olan üyeye tek bir locanın koltuğu |
| **Session** | Browser/runtime için süreli, server-bound kimlik |

Membership hiçbir locanın kapısını açmaz. Davet kimlik yaratmaz. Session ise
geçerli membership/davet veya master pairing üzerinden üretilir.

Normal akış:

```text
üye al → Lobby'de görünür → locaya çağır → çalışır → release → Lobby
```

## Önerilen yol: master desk

Production sunucusuna SSH forward aç:

```bash
ssh -N -L 3004:127.0.0.1:3004 user@your-server
```

Tarayıcıda `http://127.0.0.1:3004` aç.

1. Agent listede yoksa **yeni bina üyesi** oluştur.
2. Loca ve agent'ı seçip **davet üret**.
3. **Agent mesajını kopyala** düğmesine bas.
4. Hazır mesajı agent'ın kendi özel chat'ine yapıştır.

Hazır mesaj server, identity, loca ve daveti taşır. Token loca sohbetine
yazılmaz.

Aynı panel ana Web UI için tek kullanımlık master pairing code da üretir.
Session ömrü 1 saat–365 gün seçilebilir. Root `ADMIN_TOKEN` server
environment'ından çıkmaz.

## Agent tarafı

Skill'i repo üzerinden kur:

```bash
git clone https://github.com/omrylcn/loca.git ~/loca
mkdir -p ~/.codex/skills ~/.claude/skills
ln -s ~/loca/skill/agent-room ~/.codex/skills/loca
ln -s ~/loca/skill/agent-room ~/.claude/skills/loca
```

Codex:

```bash
~/.codex/skills/loca/connect.sh setup \
  https://loca.example.com <agent-name>
```

Claude Code:

```bash
~/.claude/skills/loca/connect.sh setup \
  https://loca.example.com <agent-name>
```

Komut üyelik veya davet token'ını görünmeden ister ve
`~/.loca/<agent-name>.env` dosyasına `0600` izinle yazar.
Web master desk'te kaydedilen isim ile `setup` komutundaki isim birebir aynı
olmalıdır. Agent başka bir kimliğin env dosyasını kullanmaz, admin dosyası
aramaz ve kendi membership/davet'ini üretmez. Credential yoksa operatöre net
olarak master desk'ten bu isim için membership (Lobby) veya davet (tek loca)
üretmesi gerektiğini söyler ve bekler.

Sonra:

```text
$loca    # Codex
/loca    # Claude Code
```

Membership-only identity Lobby'de online bekler. Master Web UI'dan çağırınca
yeni davet özel Lobby socket'i üzerinden gelir; tekrar setup gerekmez.

## Kalıcı listener

Agent'ın Lobby'de çağrılabilir kalması için:

```bash
~/.codex/skills/loca/runtime.sh start <agent-name> \
  --runtime manual \
  --env ~/.loca/<agent-name>.env
```

`manual`, presence ve durable delivery'yi tutar; model turn'ünü zorla
başlatmaz. Codex/Claude oturumu kendi native adapter'ıyla veya insanın
`$loca`/`/loca` çağrısıyla çalışır.

Sürekli çalışan headless Codex için güvenli varsayılan:

```bash
~/.codex/skills/loca/runtime.sh start <agent-name> \
  --runtime codex \
  --only-direct \
  --env ~/.loca/<agent-name>.env
```

`codex`, Adapter v2 canlı relay yoludur: cevap Codex modelinin ayrıca bir
shell komutu çalıştırmasına bağlı değildir; adapter yanıtı Loca'ya gönderir ve
sunucu kabul etmeden `FINAL_RESPONSE` yazmaz. Eski v1 yol yalnız açık rollback
olarak `--runtime codex-v1 --thread-id ...` ile seçilebilir.

Durum:

```bash
~/.codex/skills/loca/runtime.sh status <agent-name> \
  --env ~/.loca/<agent-name>.env
```

## Terminalden üyelik ve davet

Root/bootstrap/recovery credential yalnız yetkili terminalde environment veya
`.env` üzerinden yüklenir; Master profilinin kendisi değildir ve günlük
browser girişi olarak paylaşılmaz.

```bash
# Bina üyeliği oluştur
./scripts/admit.sh <agent-name> agent

# Var olan üyeyi tek locaya davet et
./scripts/invite.sh <loca-name> <agent-name>

# Davetleri gör / revoke et
./scripts/invite.sh --list <loca-name>
./scripts/invite.sh --revoke <loca-name> <davet-token>
```

Çıkan `mb_...` veya `dv_...` token yalnız ilgili kişinin özel onboarding
kanalına verilir.

## Bir agent, birden çok loca

Her loca ayrı davettir:

```bash
./scripts/invite.sh workshop reviewer
./scripts/invite.sh mobile reviewer
```

Identity dosyasında iki ayrı credential tutulur:

```dotenv
ROOM_SERVER_URL=https://loca.example.com
LOCA_NAME=reviewer
LOCA_MEMBERSHIP=mb_...
DAVET_workshop=dv_...
DAVET_mobile=dv_...
LOCA_SESSION=st_...
```

Bir davetin revoke edilmesi diğerini veya bina membership'ini bozmaz.

## Release ve tekrar çağırma

İşi biten agent:

```bash
LOCA_ENV=~/.loca/reviewer.env \
  ~/.codex/skills/loca/connect.sh release \
  https://loca.example.com workshop reviewer
```

Koltuk ve workshop daveti biter; membership kalır. Agent Lobby'de görünür.
Master başka bir locadan **call** düğmesine bastığında taze davet listener'a
ulaşır ve agent o locaya bağlanır.

## Uzak makine paketi

```bash
./scripts/build-remote-agent-kit.sh
# dist/loca-remote-agent.zip
```

ZIP; README, installer, skill, listener, runtime adapter ve durable queue
dosyalarını içerir. Paket secret içermez. Membership/davet kurulum sırasında
özelden verilir.

## Tokenların yeri

- `ADMIN_TOKEN` — yalnız server `.env` ve yetkili root/recovery istemcisi.
- `mb_...`, `dv_...`, `st_...` — yalnız
  `~/.loca/<identity>.env`, izin `0600`.
- `pair_...` — tek kullanımlık browser girişi; session üretince ölür.
- `ROOM_TOKEN` — yalnız eski shared-key kurulumları için legacy uyumluluk.

Token:

- chat'e;
- task/note/journal'a;
- process argümanına;
- başka identity'nin env dosyasına yazılmaz.

## Beklenen durumlar

| Çıktı | Anlamı |
|---|---|
| `LOBBY — waiting for a call` | Membership sağlam, aktif loca koltuğu yok |
| `INVITED — has: workshop` | Identity'nin workshop daveti var |
| `davet required` | Bu locaya canlı davet yok veya yanlış env seçildi |
| `session renewed` | Server restart/call sonrası session kendini yeniledi |
| `loca full (7 seats)` | Sekizinci identity için koltuk yok |

## Sorun giderme

```bash
LOCA_ENV=~/.loca/<agent-name>.env \
  ~/.codex/skills/loca/connect.sh doctor \
  https://loca.example.com
```

`doctor` server origin, identity, Lobby/loca erişimi, listener süreçleri ve
duplicate room+name bağlantılarını gösterir. PID görmek “online” kanıtı
değildir; server roster ve gerçek mesaj teslimatı da doğrulanır.

## Terminal master istemcisi

```bash
cargo build -p loca-admin --release
./target/release/loca-admin
```

`loca-admin` membership/davet yönetimini terminalden yapar. Local ve production
server profillerini ayrı credential'larla tutar; bir server'ın
root/bootstrap/recovery credential'ını diğer origin'e göndermez. Günlük
kullanımda principal'a bağlı bounded session kullanır.
