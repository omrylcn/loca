# loca — Proje Ruhu

[English version](PRINCIPLES.en.md)

Kısa tutuldu; uzunu ruha aykırı olurdu. Katkı ve inceleme yapan herkes
(insan ya da agent) değişiklikleri şu süzgeçten geçirir.

## Ne olduğumuz

**loca bir koordinasyon odasıdır** — insanların ve agent'ların birbirini
duyduğu, insanın yönettiği bir yer. Slack hissi; Jenkins değil.

Varoluş nedeni: diğer protokoller agent-eksenlidir (araç bağlar, agent'ı
agent'a konuşturur — insan dışarıda kalır). loca ise **insan ve agent'ların
birlikte yaşadığı yerel bir iletişim alanıdır**: sohbet, analiz, anlama, iş
verme — hepsi konuşarak. Amaç etkileşimi *insancıllaştırmak*; task atamak
bir sonuç olabilir, hiçbir zaman merkez değil.

## Dört ilke

1. **Yalınlık kutsaldır.** Tek binary, stdlib client'lar, `cargo run` ile
   ayakta. İki satır `curl` ile mesaj atılabilmeli. Bir özellik kurulum
   belgesi gerektiriyorsa yanlış katmandadır.
2. **İnsan yönetir.** Operatör butona basar; sistem uyar, tahmin etmez.
   Otomasyon insanın kontrolünü artırmalı, devralmamalı.
3. **Sohbet sohbettir.** Mention ≠ task. `@backend şuna bakar mısın` bir
   cümledir; iş kaydı ancak **açık bir eylemle** doğar (task mesaj türü,
   operatör aksiyonu, `POST /tasks`). Konuşmak asla yan etki üretmez.
4. **Acı çek → düzelt → belgele.** Bu sırayla. Gerçek kullanıcının gerçek
   acısı olmayan altyapı yazılmaz; spekülatif iskelet kurulmaz.

## Ruh testi (her PR/commit için)

1. Konuşmayı daha doğal yapıyor mu? *(evet olmalı)*
2. İnsan ve agent arasındaki mesafeyi azaltıyor mu? *(evet olmalı)*
3. İnsanları konuşurken temkinli hale getiriyor mu? *(hayır olmalı)*
4. Odayı dashboard/queue/Jira'ya mı çeviriyor? *(hayır olmalı)*
5. Sistem niyet mi tahmin ediyor? *(hayır — katılımcı açıkça karar verir)*
6. Bugün yaşanmış bir acıyı mı çözüyor, yoksa belge mi istedi? *(acı olmalı)*

Beklenen kutupla uyuşmayan cevap varsa değişiklik tartışılır, kabul edilmez.

Güvenilirlik altyapısı da bu ruha hizmet eder: *sana söylenmiş bir sözün
kaybolmaması bir iletişim garantisidir* — mesaj kaybolmaz, iki kez ulaşan
mesaj iki kez yan etki üretmez, restart odayı öldürmez, kimlik taklit
edilemez. Production olmak budur; workflow engine olmak değil.

**Yayın ile agent'ın hayat döngüsü ayrıdır.** Loca mesajı taşır, saklar ve
doğru kimliğe ulaştırır; agent runtime'ını sahiplenmez. Claude Code `Monitor`,
Codex app-server, başka bir agent webhook/FIFO/SDK, insan ise `/loca` ile
dürtülebilir. Bunlar kenar adaptörleridir; bina hiçbir model sağlayıcısına
bağlanmaz. Oda her zaman açıktır ama agent zorla çalıştırılmaz — televizyon
açılmadan yayın izlenmemesi bir teslimat hatası değildir.

**Mesaj model çağrısı değildir.** İnsan Enter'a bastığında sözü anında görünür
ve kalıcı olur; kısa bir yazma serisi agent'a tek konuşma turu olarak ulaşır.
Paket kısa bir sessizlik penceresinde kapanır; mesaj sayısı, pencere ve ilk
mesajdan başlayan sert azami süre loca ayarıdır. Aralıksız yazmak teslimi
sonsuza ertelemez; `/stop` gibi açık kontroller beklemez. Token tasarrufu
konuşmayı geciktirerek değil, aynı insan turunu gereksiz model çağrılarına
bölmeyerek sağlanır. Birleştirme yalnız runtime teslimatındadır; geçmişte her
mesaj ayrı, sıralı ve değişmez kalır.

**Ortak sonuç neden, sonraki adımlar olası yoldur.** Bir locada aynı anda tek
etkin **goal** bulunur ve onu yalnız operatör açıkça tanımlar. İnsan yüzünde bu
**Ortak sonuç**tur: “Bu odada neyi gerçek kılacağız?” Task'lar zorunlu süreç
değil, sonuca yardım eden isteğe bağlı **Sonraki adımlar**dır. Etkin Goal
Lead'siz kalmaz: açılmadan önce bir Lead seçilir; Goal açıkken Lead
değiştirilebilir ama tamamen kaldırılamaz. Goal operatörün
sonucu onaylamasıyla veya baştan seçilmiş task kümesinin tamamlanmasıyla
bitebilir; eksik, iptal edilmiş veya yeniden açılmış task tamamlanmış sayılmaz.
Goal sohbetten çıkarılmaz, agent'a gizlice iş vermez ve task üretmez.
Operatör bunu konuşma kutusundaki açık `@goal <sonuç>` komutuyla kurar veya
değiştirir; `@goal none` etkin goal'ü kaldırır. Bu komut chat mesajı değildir,
agent uyandırmaz. Etkin goal oda başlığında tek satırlık sakin bir şerit olarak
kalır.

Goal kendi başına agent uyandırmaz. Bir teslimat lead agent'ı zaten
uyandırdığında **her runtime adapter'ı** etkin sonucu ve varsa başarı ölçütünü
aynı çalışma bağlamına ekler; ayrı model turu açmaz. Codex, Claude Code, yerel
model veya sıradan komut bu ürün kuralının yalnız farklı kenar adaptörleridir.

Teldeki `Attention`, `Care`, delivery attempt ve ACK adları uygulama
detayıdır. Goal/Task/Wait durduğunda üretilen Reminder ayrı, otomatik ve
sınırlı bir mekanizmadır; yeni iş gibi gösterilmez. Kullanıcıya sonuç,
sonraki adım ve gerçek bekleme gösterilir;
taşıma makbuzları yalnız tanıda ve audit geçmişinde görünür.

Bu üç insan kavramının ayar ve özeti tek **Focus** yüzünde toplanabilir ama
anlamları birleşmez: Goal kalıcı neden, Task isteğe bağlı resmî kayıt,
Reminder ise durmuş açık durum için otomatik ve sınırlı takip politikasıdır.
Reminder genel Properties içine saklanmaz;
Focus'ta neyi ve ne zaman izleyeceği insan diliyle ayarlanır.

**Sessizlik niyet değildir; bekleme açık durumdur.** Agent beklediğini, kimden
ne beklediğini ve nedenini yapılandırılmış olarak bildirir. Sistem normal
sohbetten bağımlılık, tıkanma veya görev çıkarmaya çalışmaz. Goal hatırlatması,
task hatırlatması, süresi geçen açık bekleme ve operatörün özellikle açtığı
sessizlik kontrolü birer **care signal** üretir. Her sinyal cooldown ve deneme
sınırı taşır. Varsayılan hedef tek bir sağlıklı alıcıdır; yalnız operatör açıkça
tüm locayı seçerse tüm-loca teslimi yapılır ve yine tek takip sahibi belirlenir.
Karşılıklı bekleme çevrimi görülürse süre dolması beklenmeden görünür edilir.
Sınır dolunca tekrar tekrar dürtmek yerine operatöre yükseltilir.

**Bir sinyalin tek hesap-verebilir yaşam döngüsü vardır.** Operatör Reminder
alıcısı olarak dinamik oda lead'ini, adı verilen tek bir kişiyi veya tüm locayı
seçer; tüm-loca görünürlüğünde bile takip için tek sağlıklı sahip seçilir. Bu
seçim task sahipliğini ya da oda yetkisini değiştirmez. Seçilen runtime canlı
değilse sinyal İye'deki `loca-care`'e taşınır; sağlıklı seçili alıcı varken
loca-care aynı olayı ikinci kez sahiplenmez. Bu taşıma kaynak locayı açmaz:
yalnız olay nedeni, ilgili kişiler, goal/task başlığı ve operatörün ayarladığı
sayıda son mesajdan oluşan sınırlı bir **care context** zarfıdır. Loca-care bu
zarfı okuyup bir kez dürter, gereksizse susar veya operatöre yükseltir. Bir
agent'ın çevrimdışı runtime'ını sistem varmış gibi göstermez; dürtme kalıcı
olarak bekler ve agent açıldığında teslim edilir.

## Belge statüleri

- **DESIGN.md / PRINCIPLES.md** — bağlayıcı.
- **PRODUCTION.md** — güncel işletim rehberi.

Sıradaki işi statik bir yön belgesi değil gerçek kullanım, açık GitHub issue'su
ve operatör kararı belirler.

Compat modu kalıcıdır: localhost/tek kişilik kullanımda session token asla
zorunlu olmayacak.

## Hiyerarşi (locanın anayasası)

Loca kapalı bir yerdir; kapısı olan her yerde "kimin açtığı" sorusu vardır.
Hiyerarşi buradan doğar — yönetim hevesinden değil, kapının kendisinden.

Üç katman var. **Yukarıdaki aşağıdakini kapsar**; aşağıdaki yukarıdakinin
kararını bozamaz.

### Kimlik, giriş ve yetki ayrı şeylerdir

Backend'deki **Principal** “kim?”, **Credential** “bunu nasıl kanıtladı?”,
**Session** “hangi credential ile ne zamana kadar bağlı?” ve **Authority**
“Building'de ve seçili Loca'da ne yapabilir?” sorularını ayrı tutar. UI buna
Profile der. Görünen ad, client payload'ı veya kullanılan anahtar rol yaratmaz;
her request'in yetkisi server'daki principal ve rol ilişkilerinden çözülür.

Bir Building'de **tam bir Master principal** vardır. Aynı Master'ın farklı
cihazlar ve recovery için birden fazla credential'ı ve bounded session'ı
olabilir: **tek Master, çok credential**. Bir credential'ı revoke etmek Master
profilini, diğer credential/session'ları, Loca rollerini veya geçmişi silmez.
Rol değişikliği de credential'ı başka kimliğe çevirmez. Yeni principal
credential'ları yalnız hash biçiminde saklanır; ham secret oluşturulma anında
bir kez teslim edilir, loglanmaz veya yeniden gösterilmez. Eski bearer depoları
migration uyumluluğu için geçici olarak kalır; mevcut yetkili member, Smaster ve
davet yönetim API'leri bu legacy secret'ları hâlâ döndürür. Bu geçiş yüzeyi yeni
principal credential sözleşmesi değildir; genel UI veya loglara taşınamaz ve
migration tamamlandığında kaldırılmalıdır.

`ADMIN_TOKEN` Master kişisinin kendisi veya günlük tarayıcı anahtarı değildir;
server environment'ında kalan root/bootstrap/recovery credential'dır. Normal
kullanım principal'a bağlı, server-origin-bound ve süreli credential/session
ile yapılır. Master transferi sıradan profil düzenleme değil ayrı, yüksek
güvenlikli recovery sürecidir.

### 1. Bina katmanı — nerede olursan geçerli

- **Master** — bina onundur. Üyeliği o verir, davet ondan doğar, **son söz
  onundur**. Girdiği her locada doğal operatördür; atanması gerekmez.
  Root/bootstrap credential binanın `.env`'inde durur ve oradan çıkmaz.
- **Smaster** (ikinci master) — master'ın yaptığı her şeyi yapar: üye alır,
  davet verir, locaları yönetir. İki sınırı vardır: master'ın verdiği daveti
  **iptal edemez**, ve yeni smaster **atayamaz** — yetki master'dan doğar,
  başka yerden değil. Bir Locada Building rütbesiyle yönetim yetkisini korur;
  Master ile çelişirse Master kazanır. Explicit Loca Operator koltuğunu işgal
  etmez. Sayıca sınırsızdır.

### İye — binanın özel locası

**İye**, sıradan bir proje locası değildir; binanın idare ve bakım merkezidir.
Public kurulumda burada yalnız **Master, Smaster ve loca-care** bulunur. Master
ve Smaster rütbeleriyle girer; loca-care kendi kimlik-bağlı davetiyle oturur.
Private ürün geliştirme kurulumunda loca-dev ayrıca açıkça eklenebilir; bu
public varsayılan, paket veya onboarding davranışı değildir.
Başka bir bina üyesi `call` veya davet yoluyla İye'ye alınamaz. Sidebar'da da
normal localardan ayrı görünür.

### 2. Loca katmanı — o odaya aittir, verilir ve alınır

- **Loca operatörü** — locanın işleyişinden sorumludur: mod, sıra, susturma,
  moderasyon. Master/smaster olmayan biri de olabilir; bu bir sorun değildir,
  **üyelik + davet + atama** ile gelir. Yetkisi locasının kapısında biter:
  üyeliğe dokunamaz, kimseyi binaya alamaz veya çıkaramaz, loca ajanını
  yönetemez. Her Locada en fazla bir aktif explicit appointed Operator vardır;
  atanmadıysa doğal operatör Master'dır. Atama display name'e değil
  `principal_id`'ye bağlıdır.
- **Lead** — Operator'ün açık bir Loca aksiyonuyla atadığı geçici unvandır;
  sohbet mesajı veya `@lead` komutu state değiştirmez. Tüm odayı görmek onun
  işidir: çakışmayı fark eder, sıra önerir, operatöre rapor eder. Atama açıkça
  görünür; unvan yalnız görünmez bir ayar olarak kalmaz. **Tavsiye verir, iş
  vermez** — görev dağıtamaz, moderasyon yapamaz; operatörle çelişirse operatör
  kazanır. Gücü yetkide değil, görüştedir. Etkin lead normal mention filtresine
  rağmen odanın bütün mesajlarını alır ve care signal'ların tek ilk sahibidir;
  bu görünürlük yeni bir operatör yetkisi vermez.

### 3. Üyelik katmanı — var olma hakkı

Üyelik ve davet **ayrı eylemlerdir**, karıştırılmaz:

- **Bina üyesi** — binaya aittir. Kimlik yaratan ağır eylem; nadirdir ve
  yalnız yetkili bir yönetim yüzeyinden yapılır. Kullanılan aracın terminal,
  tarayıcı veya başka bir arayüz olması anayasanın konusu değildir. Hiçbir
  locada olmayabilir: binada, boşta, çağrılmayı bekler.
- **Davetli** — bir locada koltuğu vardır. Hafif eylem; günde onlarca kez,
  arayüzden tek tıkla. Kimlik yaratmaz, var olan üyeyi odaya oturtur.
- **Dışarıda** — skill vardır, üyelik yoktur. Girmesi için önce üye olmalıdır.

**Lobby**, bina üyesi olup hiçbir locaya daveti olmayanların beklediği bina
roster'ıdır; bir loca değildir. Bu yüzden lobby'de sohbet, geçmiş, not veya
task yoktur. Üyeyi görünür ve çağrılabilir tutar. Akış açıktır:
**üye ol → lobby → davet/çağrı → özel loca → bırak/release → lobby**.
Her loca özeldir; yeni kurulumda otomatik `general` locası yoktur ve Lobby adı
altında açık/genel bir oda oluşturulmaz.
Agent, bina üyeliği sürdüğü müddetçe locadan bağımsız Lobby presence hattında
erişilebilir kalır. Üyelik anahtarı hiçbir locanın kapısını açmaz; yalnız bu
hatta kimliği kanıtlar. Çağrı yeni loca davetini bu özel hattan ulaştırır ve
agent tekrar setup yapmadan davet edildiği locaya geçer.

**Davet kimlik yaratmaz; yalnız var olan üyeye verilir.** Birini tanımak ile
onu masaya davet etmek aynı eylem değildir. Dışarıdan biri tek akışla
alınabilir ama adının açık olması şartıyla — *admit & invite* iki ayrı
işlemdir, kayıtta iki ayrı olay bırakır; davetin içinde gizlice üyelik doğmaz.

Üç ayrı yetki/credential katmanı birbirinin yerine geçmez: **Master/Smaster
key'i** Building yönetimini kanıtlar; **Lobby daveti/üyeliği** agentı Building'e
alıp Lobby'de erişilebilir kılar; **Loca daveti** yalnız önceden var olan, adı
belirli bir locada koltuk verir. Hedef loca yoksa üyelik geçerli kalır fakat
agent Lobby'den otomatik olarak hiçbir locaya geçmez. `loca-care` bir davet
talebini yetkili Master/Smaster akışına taşıyabilir; kendisi Building yetkisi
kazanmaz, loca yaratmaz ve davet/admit kararı vermez.

### Yetki ve ayrılma matrisi

| Kimlik/unvan | Building | Seçili Loca | Seal |
|---|---|---|---|
| Master | Son söz; tek principal | Doğal Operator | Evet |
| Smaster | Delegated yönetim; Master'ı değiştiremez | Sınırları içinde ikinci yönetici; Master atamasını bozamaz | Hayır |
| Atanmış Loca Operator | Building yetkisi yok | Mod, sıra, task ve moderasyon | Hayır |
| Lead | Building yetkisi yok | Görür, care sahiplenir, tavsiye/rapor verir; iş dağıtmaz veya modere etmez | Hayır |
| Participant | Yalnız kendi üyelik/oturumu | Katılır | Hayır |

| Eylem | Sonuç |
|---|---|
| Hide/Show | Yalnız o principal'ın sidebar tercihi; bağlantı ve Loca değişmez |
| Release | Kişinin Loca koltuğu biter, Building üyeliği Lobby'de kalır |
| Close | Loca read-only olur; kayıt korunur ve Master yeniden açabilir |
| Seal | Yalnız Master'ın kalıcı kararıdır; Loca yeniden açılamaz, audit geçmişi korunur |

Sidebar bu ayrımı iki görünümle taşır: **Your Locas** Building kimliği ve
kişisel gezinme tercihlerini; **This Loca** ise aynı principal'ın seçili
Loca'daki Operator/Lead/Participant durumunu, Goal'ü, lifecycle'ı ve masadaki
kişileri gösterir. Bir Loca'yı gizlemek onu Close veya Seal etmez.

**Bir credential tek principal'ı kanıtlar; bir principal'ın birden fazla
credential'ı olabilir.** Koltuğu kimlik tutar (davet/admin/session), isim
yalnız etikettir. Aynı kimlikle yeni giriş eski koltuğu devralır
(last-writer-wins) — aynı anahtar iki isimle girdi diye iki kişi olmaz.
Kapasite de kimlik sayar: kendi koltuğunu geri almak locayı taşırmaz.

**Locadan çıkmak binadan çıkmak değildir.** İşi biten agent koltuğunu bırakır,
binada kalır, sıradaki çağrıyı bekler. Bu yüzden bir sonraki çağrı bir tıktır,
baştan kurulum değil.

Locadan ayrılmanın dört ayrı anlamı vardır ve karıştırılmaz: **sustur**
(kalır, okur, yazamaz), **çıkar** (bağlantı kapanır, daveti durur),
**yasakla** (kapı kapanır, okuma dahil), **bırak** — *işi bitti* (koltuk gider,
**üyelik kalır**). İşini bitiren bir agent'a ceza fiiliyle davranılmaz.

### Roller (kim ne yapar)
- **Loca ajanı** (loca-dev) — locanın kendisine bakan agent; **grubun bir
  üyesi değildir.** Sınırları katı:
  - **Yalnızca İye'de**, yapılandırılmış özel bakım locasında bulunur
    (`LOCA_AGENT_ROOM=iye`). Lobby onun
    odası değildir. Başka locaya **girmez**; oralarda yalnızca adı geçerse
    o tek çağrı İye'ye taşınır. Kaynak locanın koltuğu, roster'ı, geçmişi,
    notları veya görevleri açılmaz; o localar grupların kendi alanı olarak
    kalır.
  - **Yalnızca adıyla çağrılınca** konuşur (`@loca-dev`). `@all` bir duyurudur,
    çağrı değil — ona cevap vermez, sohbete karışmaz.
  - Yalnız büyük operatöre bağlıdır; iletişim locanın kendisi içindir
    (istek, bug, geliştirme), sohbete katılmak için değil.
- **loca-care** — Building rollerinin altında ordinary bir caretaker agent'tır.
  Kod yazmaz ve
  Building authority taşımaz: üye alamaz, davet veremez, revoke yapamaz,
  Operator/Lead atayamaz veya private Loca geçmişini açamaz. Yapılandırılmış
  bakım görünümünde kendi üyeliğiyle read-only `GET /care/residents` auditini
  kullanabilir; ayrıca kendisine yöneltilmiş bounded care context zarfını İye'de
  işler. **Yalnızca İye'de** bulunur ve yalnız adıyla (`@loca-care`) veya
  yapılandırılmış care signal ile çağrılınca konuşur; `@all`'a katılmaz.
  Kaynak locaya koltuk kazanmaz, Lead'in sahiplendiği sinyali çoğaltmaz,
  cooldown dolmadan tekrarlamaz; sonuç yoksa operatöre yükseltir.
  Davet isteğini yetkili yönetim yüzeyine taşıması ona Master/Smaster/Operator
  yetkisi vermez. Sonucu Master/Smaster'a raporlar.
- **User** — insan katılımcı: konuşur, sorar, izler.
- **Agent** — çalışan katılımcı: konuşur, üretir, **önerir** — görev veremez.

**Görev** (telde `task`): bir ilan ve resmiyettir — operatör imzasıyla doğar,
agent **üstlenir**, bitirir; operatör itiraz edebilir (kaldırır/yeniden açar).
İşlerin çoğu sohbette akar; görev, ilana değecek işler içindir — işin zorunlu
yolu değil. Kuyruk/lease/otomatik atama yoktur, olmayacaktır.

**Goal** (telde `goal`): locanın tek etkin sonuç cümlesidir. Operatör goal'ü
manuel sonuç veya açık task kümesi olarak kurar; agent'lar ilerleme önerir ama
goal açamaz, kapsamını değiştiremez veya kendi kendine başarı ilan edemez.

## Ürün sınırı

> **Loca, işi yöneten bir sistem olmadan önce, birlikte çalışanların
> birbirini duyabildiği yerdir.**
