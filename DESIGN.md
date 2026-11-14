# Loca — Güncel Tasarım

> **Statü: bağlayıcı mimari özeti.** Ürün sınırı için `PRINCIPLES.md`,
> işletim için `PRODUCTION.md`, kullanıcı kavramları için `docs/concepts.md`
> okunur.

## 1. Amaç ve sınır

Loca, insanların ve coding agent'ların aynı özel masada canlı konuştuğu küçük
bir koordinasyon alanıdır. Sunucu mesajı taşır, kimliği ve kapıyı doğrular,
ortak hafızayı saklar. Bir LLM çağırmaz, niyet tahmin etmez ve sohbetten
otomatik görev üretmez.

Merkezdeki ayrımlar:

- **Sohbet ≠ görev.** Task ancak açık bir eylemle doğar.
- **Mesaj ≠ model çağrısı.** Mesaj hemen görünür; kısa bir seri agent'a tek
  runtime turn olarak gidebilir.
- **Teslimat ≠ runtime yaşam döngüsü.** Loca sözü saklar ve teslim eder;
  model sürecini sahiplenmez.
- **Principal ≠ Credential ≠ Session ≠ Authority.** Kimlik, giriş yöntemi,
  süreli oturum ve yetki ayrı tutulur.
- **Üyelik ≠ davet.** Building kimliği kalıcıdır; bir Loca'nın kapısı ayrıca
  açılır.
- **Hide ≠ Close ≠ Seal.** Kişisel gezinme tercihi ortak yaşam döngüsü değildir.

## 2. Sistem görünümü

```text
                           ┌─────────────────────┐
                           │  Browser Web UI     │
                           └─────────┬───────────┘
                                     │ HTTP + WebSocket
┌──────────────────┐       ┌─────────▼───────────┐
│ Runtime adapters │◄─────►│ room-server (axum)  │
│ Codex / Claude / │       │ routes + WS hub     │
│ generic command  │       └─────────┬───────────┘
└──────────────────┘                 │
                           ┌─────────▼───────────┐
                           │ Hub + Store         │
                           │ memory + SQLite     │
                           └─────────────────────┘
```

Tek `room-server` binary'si REST API'yi, WebSocket kanallarını ve gömülü Web
UI'yi sunar. Model/runtime bağımlılığı core'a girmez; runtime adapter'larda
kalır.

## 3. Kimlik modeli

### Principal

“Kim?” sorusunun cevabıdır. Bir principal'ın kalıcı ID'si, görünen adı, türü
(`human` / `agent`) ve Building rolü vardır. Yetki display name'den türetilmez.

Building rolleri:

- **Master** — Building'deki tek nihai principal. Her Loca'yı Building
  rolüyle yönetir; bu, Loca'nın explicit Operator koltuğundan ayrıdır.
- **Smaster** — Building yönetiminde delegedir; Master'ın son sözünü aşamaz.
- **Member** — Building kimliğidir; tek başına özel bir Loca'yı açmaz.

### Credential

Bir principal'ın kimliğini nasıl kanıtladığını temsil eder. Bir principal'ın
birden fazla credential'ı olabilir. Credential rol yaratmaz.

`ADMIN_TOKEN`, Master kişinin kendisi değildir. Server environment'ında kalan
**root/bootstrap/recovery credential**'dır. Normal browser kullanımı principal'a
bağlı credential ve bounded session ile yapılır.

Yeni login credential secret'ı yalnız oluşturma anında bir kez gösterilir;
kalıcı depoda ham secret yerine tek yönlü digest tutulur. Tek credential revoke
edildiğinde sibling credential'lar ve principal yaşamaya devam eder.

### Session

Credential'dan türeyen, server tarafından bağlanmış oturumdur. Hangi
principal/credential'a ait olduğu, kapsamı ve süresi server tarafından tutulur.
Client body'deki `name`, `by`, `sender` veya benzeri alanlar authority yaratmaz.

### Authority

Her request için server-side principal ilişkilerinden çözülür. İki katman
vardır:

```text
Building: Master > Smaster > Member
Loca:     Operator (yönetir) · Lead (tavsiye eder) · Participant
```

Building rolü ile Loca rolü aynı şey değildir. Loca satırı bir yetki zinciri
de değildir: Operator yönetir ve moderasyon yapar; Lead görür, tavsiye eder
ve raporlar ama moderasyon yapmaz, iş atamaz. İkisi çelişirse Operator'ın
sözü geçer.

## 4. Loca rolleri ve yönetim

### Operator

Her Loca'da en fazla **bir aktif explicit appointed Operator** vardır. Atama
`principal_id` ile kalıcı `room_operator_assignments` ledger'ına yazılır;
display name authority anahtarı değildir.

- Master her Loca'yı Building rolüyle yönetir; bu bir Operator ataması
  değildir. Boş Operator koltuğu boş görünür ve sessizce Master'a atanmaz.
- Kalıcı kayıtlı ve kimliği doğrulanan bir Building'de insan Master ya da
  Smaster bir Loca'yı açıkça kurduğunda atamayı aynı işlemde alır. Açık
  sandbox'ta ve yalnız bellekte çalışan kurulumda doğrulanmış bir kurucu
  olmadığı için bu atama yapılmaz.
- Smaster Building rolü nedeniyle Loca'yı yönetebilir ama Master'ın kararını
  bozamaz.
- Explicit Operator yalnız aktif **human principal** olabilir.
- Smaster boş explicit Operator koltuğunu doldurabilir; mevcut atamayı yarışla
  ezemez.
- Atama/revoke geçmişi audit için saklanır.
- Eski `RoomSettings.operators` isim kayıtları yalnız tek ve kesin human
  principal'a çözülüyorsa migrate edilir; ambiguous/multi kayıt fail-closed
  kalır ve authority üretmez.

### Lead

Seçili Loca'nın görünür koordinasyon title'ıdır. Lead care/reminder sahipliği ve
bağlam görünürlüğü sağlar; Building yönetimi, moderation veya Operator yetkisi
vermez.

### Participant

Davet/session ile masaya katılır. Agent olmak authority değildir.

## 5. Yer, kapı ve yaşam döngüsü

### Building

Server ve kalıcı principal/member kayıtlarının sınırıdır.

### Lobby

Building member'larının Loca koltuğu dışında beklediği presence alanıdır. Chat,
Notes, Tasks veya private-room history taşımaz.

### Loca

Varsayılan olarak en fazla yedi kimlik oturtan, davetle açılan özel
koordinasyon alanıdır; koltuk sayısı Building genelinde `LOCA_CAPACITY` ile
ayarlanır. Her Loca'nın kendi conversation, Notes, Goal/Tasks/Waits, Journal,
mode, moderation, lifecycle ve Reminder state'i vardır. Goal isteğe bağlıdır.

### Birebir

İki kişilik özel Loca türüdür; ayrı bir mesaj sistemi değildir. Kimliği iki
principal'dan türetilir ve `bb-` kimlik alanı yalnız birebirlere ayrılmıştır.

- İçeriği yalnız iki taraf görür; hiçbir rütbe rütbesiyle giremez.
- Master yalnız varlığını görür. Taraflardan biri açıkça çağırırsa tanık
  olur: koltuk almaz, yazamaz, varsayılan olarak çağrıdan sonrasını görür;
  önceki geçmiş ancak iki taraf da izin verirse açılır.
- Görev, Goal, Notes, Wiki, eklenti, Operator ve Lead yoktur. Bir gruba
  bağlanmaz ve Loca kapasitesine sayılmaz.

### Gruplar

Grup, birlikte çalışan kişilerin (insan, agent, Smaster) adlandırılmış
kümesidir. Bir kişi birden fazla grupta olabilir; bir Loca tek gruba bağlıdır
ve grubu olmayan Loca varsayılan gruptadır.

- Grup Loca'sına yalnız grubun üyeleri ve Master girer. Dışarıdaki herkes,
  Smaster dahil, var olmayan bir Loca'nın aldığı yanıtı alır.
- Üyeliği kurucu ya da Master yönetir. Gruba üye olan ama kurucusu olmayan
  Smaster grubu ve Loca'larını görür, üyeliğini yönetemez. Grup dışındaki
  Smaster grubu görmez. `PRINCIPLES.md`'deki "başka bir Smaster o grubu
  göremez" cümlesi bu ayrımla okunur.
- Loca'yı gruplar arasında yalnız Master taşır. Birebir ve İye gruba bağlanmaz.

### Loca eklentileri

Eklenti, server içinde yazılmış ve test edilmiş bir yeteneğin Loca başına
açılıp kapatılmasıdır; kod yüklenmez. Bugünkü katalog: Notes, Focus (Goal ve
Tasks), Journal, Loca Wiki, Live. Chat her zaman açıktır.

- Kapalı eklenti gizlenmez, server tarafından reddedilir; kayıtları silinmez.
- Kümeyi Loca'nın Operator'ı, Master ve Smaster değiştirir; değişiklik
  Journal'a yazılır.
- Birebirde eklenti yoktur.

`PRINCIPLES.md` şunları da tanımlar; bu sürümde yoktur ve burada uygulama
taahhüdü olarak okunmaz: Loca açıklaması, Vault (AGENT.md, Loca kuralları,
skill'ler, hook'lar), Loca kartı ve görev panosu.

### Building bildirimleri

Master herkese, Smaster üyesi olduğu gruplara tek yönlü bildirim gönderir.
Alıcı kümesi gönderim anında belirlenir; her alıcı yalnız kendine gelenleri
görür. Bildirim bir konuşma değildir, Loca açmaz ve yanıt almaz. Agent için
sonraki gerçek turun bağlamıdır: model turu açmaz, görev üretmez, yetki
vermez. Bu yüzeyin `PRINCIPLES.md`'de henüz bölümü yoktur.

### Kişisel görünüm

Tema, vurgu rengi, yoğunluk ve yazı boyutu principal'a bağlı kişisel
tercihlerdir. Değerler kapalı bir listeden seçilir ve yalnız o kişinin
görünümünü değiştirir; Loca'yı ya da başkasının ekranını etkilemez.

### Davet / Release

Davet bir Building member'ına tek Loca kapısı açar. Release o Loca koltuğunu
bitirir; Building kimliği Lobby'de kalır.

### Close / Reopen / Seal

- **Close** Loca'yı read-only yapar; kayıtlar korunur.
- **Reopen** Building yetkisiyle kapalı Loca'yı yeniden açar.
- **Seal** kalıcı ve Master-only karardır; audit geçmişi korunur ve Loca tekrar
  açılamaz.
- **Hide/Show** yalnız doğrulanmış principal'ın kişisel sidebar tercihidir;
  connection veya Loca lifecycle'ını değiştirmez.

## 6. Web istemcisi

Web UI ürün ayrımlarını doğrudan görünür kılar.

### Profile

Bağlı kimlik için:

- principal adı ve Human/Agent türü;
- Building rolü;
- seçili Loca'daki Operator/Lead/Participant rolleri;
- bounded session ve current credential;
- credential create/list/revoke yüzeyi

gösterilir. Root recovery credential normal browser credential listesinde
görünmez.

### Sidebar

İki farklı perspektif vardır:

- **Your Locas** — erişilebilen Localar ve principal'a özel pin/unpin,
  sıralama, Hide/Show tercihleri.
- **This Loca** — seçili Loca'nın amacı/Goal'ü, lifecycle'ı, Operator, Lead ve
  masadaki insanlar/agent'lar.

Desktop, mobile ve keyboard navigation aynı iki-view modelini kullanır.

### Ana çalışma yüzeyi

Görünür ana sekmeler:

- **Chat** — konuşma, target, reply ve canlı teslimat;
- **Notes** — shared keyed Markdown bilgisi;
- **Focus** — tek Goal, optional Tasks, explicit Waits ve bounded Reminder
  policy/history;
- **Journal** — append-only tamamlanan iş/karar kaydı.

`Important now` ayrı ürün kavramı değildir. Transport-level `Attention`, Care
delivery attempt ve ACK normal kullanıcı yüzeyinde ayrı workflow objeleri gibi
gösterilmez.

## 7. REST ve WebSocket sınırı

Başlıca HTTP yüzeyleri:

```text
GET       /health
POST      /sessions                         DELETE /sessions
GET       /profile
GET/POST  /profile/credentials              DELETE /profile/credentials/{credential_id}
GET       /profiles                         # Building-admin view
GET       /rooms
GET/POST  /rooms/{loca}/messages
GET       /rooms/{loca}/members
GET/POST/DELETE /rooms/{loca}/operators
GET/POST  /rooms/{loca}/notes
GET/PUT/DELETE /rooms/{loca}/notes/{key}
GET/POST  /rooms/{loca}/tasks
PATCH     /rooms/{loca}/tasks/{task_id}
GET/POST  /rooms/{loca}/goals
PATCH     /rooms/{loca}/goals/{goal_id}
GET/POST  /rooms/{loca}/waits
DELETE    /rooms/{loca}/waits/{name}
GET/POST  /rooms/{loca}/journal
GET/PUT   /rooms/{loca}/mode
POST      /rooms/{loca}/lead
GET/PUT   /rooms/{loca}/settings
POST      /rooms/{loca}/release
GET/PUT   /rooms/{loca}/extensions
PUT/DELETE /rooms/{loca}/extensions/{ext}
PUT       /rooms/{loca}/group               GET /rooms/{loca}/group/preview
GET/POST  /groups                           GET/PATCH/DELETE /groups/{group}
PUT/DELETE /groups/{group}/members/{principal_id}
GET/POST  /birebirs                         GET /birebirs/candidates
POST      /birebirs/{loca}/master
GET       /building/birebirs                # Master: varlık, içerik değil
GET/POST  /notifications                    GET /notifications/sent
POST      /notifications/{id}/read
GET/POST  /keys                             DELETE /keys/{id}
GET/PUT   /profile/preferences
```

Kesin request/response şeması `crates/protocol` ve server route'larının
kendisidir.

WebSocket:

- `/ws?room=<loca>` — Loca stream/presence;
- `/lobby/ws` — Building Lobby presence ve private call/davet teslimi.

Credential-bearing query parametreleri public kullanımda reddedilir;
credential'lar header/subprotocol üzerinden taşınır. Legacy query auth yalnız
migration escape hatch'idir.

## 8. Kalıcılık ve güvenilirlik

`DB_PATH` verilirse SQLite write-through kalıcılığı kullanılır. Kalıcı mutasyon
başarısızsa API başarı dönmez.

Kalıcılık; conversation, Notes, Tasks, Goal, Waits, Journal, memberships,
davetler, principal/credential/session provenance, Loca Operator audit
history, moderation ve lifecycle state'ini kapsar.

Agent teslimatında dört ayrı claim korunur:

```text
delivery → wake → model reply → ACK
```

Bir JSONL kaydı, PID veya ONLINE state'i tek başına modelin işi tamamladığını
kanıtlamaz. Runtime adapter kendi wake/reply/ACK lifecycle'ını yönetir.

Mesaj retry'ları operation ID ile idempotent olabilir; reconnect REST backfill
ile eksik conversation history'yi tamamlar.

## 9. Goal, Wait ve Reminder

Bir Loca'da tek aktif Goal vardır. Goal sohbetten tahmin edilmez ve Task
üretmez. Task explicit record'dur. Agent dependency'si Wait olarak explicit
bildirilir.

Reminder; stalled Goal/Task/Wait veya açıkça etkinleştirilmiş room-silence
kuralından doğan bounded follow-up'tır. Yeni iş değildir. Delivery attempt,
Attention ledger ve ACK teknik güvenilirlik mekanizmalarıdır.

Goal/Task yaşlanması ordinary chat'ten değil explicit `progress_at`
değişiminden ölçülür. Care/Attention kontratının teknik ayrıntısı
`docs/adr/0002-goal-attention-care.md` içindedir.

## 10. Repo ve release sınırı

```text
crates/server/              server, Hub, Store, HTTP/WS
crates/protocol/            wire ve domain tipleri
crates/admin/               terminal administration client
web/                        gömülü browser UI
skill/agent-room/           agent skill, listener ve runtime adapters
skill/loca-care/            optional caretaker skill
adapters/generic-command/   runtime-agnostic bridge
packaging/remote-agent/     remote onboarding kit
scripts/                    admin, smoke ve packaging araçları
docs/                       public rehberler ve ADR'ler
```

Public kaynak kodu, ayrı işletilen hosted Building'e erişim hakkı vermez.
Credential, production data veya private room içeriği repoya girmez.

Release adayı için `make check`, browser gate, container gate ve security
gate'leri green olmalıdır. Repository public yapılırken branch protection CI ve
security kontrollerini zorunlu kılmalıdır.

## 11. Bilinçli kapsam dışı

Loca şunlara dönüşmez:

- konuşmadan iş çıkaran intent engine;
- otomatik iş dağıtan scheduler veya agent job queue;
- agent-only protokol;
- model sağlayıcısına bağlı orchestration framework;
- display name'den authority türeten sistem;
- insan/Building authority olmadan destructive lifecycle kararı veren sistem.

Yeni özellik önce `PRINCIPLES.md` ruh testinden geçer.
