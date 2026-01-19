# Suderra Edge Agent - Kullanım Senaryoları

Bu dokümanda Suderra Edge Agent'ın gerçek dünya uygulamalarındaki kullanım senaryoları anlatılmaktadır.

---

## Senaryo 1: Balık Çiftliği Su Kalitesi İzleme

### Durum
Bir balık çiftliğinde 10 adet havuz bulunmaktadır. Her havuzda su sıcaklığı, oksijen seviyesi ve pH değeri sürekli izlenmeli, kritik değerlerde alarm verilmelidir.

### Çözüm

```
┌─────────────┐     Modbus      ┌──────────────┐      MQTT       ┌─────────────┐
│  Sıcaklık   │────────────────►│              │────────────────►│             │
│  Sensörü    │                 │   Suderra    │                 │   Cloud     │
├─────────────┤                 │    Edge      │                 │  Platform   │
│   Oksijen   │────────────────►│   Agent      │◄───────────────│             │
│   Sensörü   │                 │              │   Komutlar      │             │
├─────────────┤                 │              │                 │             │
│  pH Sensörü │────────────────►│              │────────────────►│  Dashboard  │
└─────────────┘                 └──────────────┘      Alarmlar   └─────────────┘
```

### Nasıl Çalışır?

1. **Sabah 06:00** - Sistem otomatik başlar
   - Edge agent, tüm sensörlere bağlanır
   - Her 10 saniyede bir ölçüm alır
   - Veriler cloud'a gönderilir

2. **Öğlen 12:30** - Sıcaklık yükselir
   - Su sıcaklığı 26°C'den 29°C'ye çıkar
   - Agent otomatik alarm üretir
   - Telefona bildirim gider
   - Soğutma sistemi devreye girer

3. **Gece 02:00** - İnternet kesilir
   - Agent çalışmaya devam eder
   - Veriler yerel olarak saklanır (offline queue)
   - Bağlantı gelince otomatik gönderilir

### Script Örneği

```json
{
  "id": "yuksek-sicaklik-alarm",
  "name": "Yüksek Sıcaklık Alarmı",
  "triggers": [
    {
      "trigger_type": "threshold",
      "source": "havuz_1_sicaklik",
      "operator": "gt",
      "value": 28.0
    }
  ],
  "actions": [
    {
      "action_type": "alert",
      "level": "warning",
      "message": "Havuz 1 sıcaklığı yüksek: ${havuz_1_sicaklik}°C"
    },
    {
      "action_type": "webhook",
      "url": "https://hooks.slack.com/services/XXX",
      "message": "🚨 Sıcaklık Uyarısı: Havuz 1 = ${havuz_1_sicaklik}°C"
    }
  ]
}
```

---

## Senaryo 2: Sera Otomasyonu

### Durum
Bir serada sıcaklık, nem ve toprak nemi izlenmekte, havalandırma fanları ve sulama sistemi otomatik kontrol edilmektedir.

### Günlük Akış

```
06:00  Güneş doğar
       ├── Sıcaklık sensörü: 18°C
       ├── Agent beklemede
       └── Fan kapalı

10:00  Sıcaklık yükselir
       ├── Sensör: 32°C (eşik: 30°C)
       ├── Agent fan çalıştırır (GPIO pin 17 = HIGH)
       └── Cloud'a "fan_acildi" mesajı gider

12:00  Toprak nemi düşük
       ├── Nem sensörü: %25 (eşik: %30)
       ├── Agent sulama pompasını açar
       ├── 5 dakika çalışır
       └── Otomatik kapanır

18:00  Sıcaklık düşer
       ├── Sensör: 26°C
       ├── Hysteresis: 28°C altına düşmeli (deadband: 2°C)
       ├── Henüz kapanmaz (titreşim önleme)
       └── 25°C'de fan kapanır
```

### Kullanılan Özellikler

| Özellik | Kullanım |
|---------|----------|
| **Threshold Trigger** | Sıcaklık eşik aşımı tespiti |
| **HYSTERESIS FB** | Fan açma/kapama titreşimini önler |
| **TON Timer** | Sulama süresini kontrol eder |
| **GPIO Write** | Fan ve pompa kontrolü |
| **Webhook** | Telefona bildirim |

---

## Senaryo 3: Fabrika Enerji İzleme

### Durum
Bir fabrikada 5 adet üretim makinesi bulunmaktadır. Her makinenin enerji tüketimi izlenmeli, toplam tüketim raporlanmalı ve pik saatlerde uyarı verilmelidir.

### Sistem Yapısı

```
┌─────────────────────────────────────────────────────────────┐
│                      FABRİKA KATI                           │
│                                                             │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐       │
│  │Makine 1 │  │Makine 2 │  │Makine 3 │  │Makine 4 │       │
│  │ Enerji  │  │ Enerji  │  │ Enerji  │  │ Enerji  │       │
│  │ Sayacı  │  │ Sayacı  │  │ Sayacı  │  │ Sayacı  │       │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘       │
│       │            │            │            │             │
│       └────────────┴─────┬──────┴────────────┘             │
│                          │                                  │
│                    ┌─────▼─────┐                           │
│                    │   PLC     │                           │
│                    │ (Modbus)  │                           │
│                    └─────┬─────┘                           │
│                          │                                  │
└──────────────────────────┼──────────────────────────────────┘
                           │
                     ┌─────▼─────┐
                     │  Suderra  │
                     │   Edge    │
                     │   Agent   │
                     └─────┬─────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
        ┌─────▼─────┐ ┌────▼────┐ ┌────▼────┐
        │  Cloud    │ │ Slack   │ │ Grafana │
        │ Platform  │ │ Webhook │ │ Metrics │
        └───────────┘ └─────────┘ └─────────┘
```

### Özellikler

1. **MAVG (Moving Average)** - Enerji tüketim ortalaması
   - Son 10 ölçümün ortalaması alınır
   - Ani değişimler filtrelenir
   - Trend analizi yapılır

2. **PID Controller** - (Gelecek: Klima kontrolü)
   - Fabrika sıcaklığı sabit tutulur
   - Enerji optimizasyonu sağlanır

3. **Cron Trigger** - Günlük rapor
   - Her gün 23:59'da toplam tüketim hesaplanır
   - Cloud'a rapor gönderilir

### Script: Pik Saat Uyarısı

```json
{
  "id": "pik-saat-uyarisi",
  "name": "Pik Saat Enerji Uyarısı",
  "triggers": [
    {
      "trigger_type": "schedule",
      "cron": "0 17-21 * * 1-5"
    }
  ],
  "conditions": [
    {
      "condition_type": "sensor",
      "source": "toplam_enerji_kw",
      "operator": "gt",
      "value": 500
    }
  ],
  "actions": [
    {
      "action_type": "alert",
      "level": "warning",
      "message": "Pik saatte yüksek tüketim: ${toplam_enerji_kw} kW"
    },
    {
      "action_type": "set_variable",
      "target": "pik_uyari_sayisi",
      "value": "${var:pik_uyari_sayisi} + 1",
      "scope": "retain"
    }
  ]
}
```

---

## Senaryo 4: Su Pompası İstasyonu

### Durum
Bir su pompası istasyonunda 3 adet pompa bulunmaktadır. Tank seviyesine göre pompalar sırayla devreye girmeli, arıza durumunda yedek pompa çalışmalıdır.

### Çalışma Mantığı

```
Tank Seviyesi    Pompa 1    Pompa 2    Pompa 3 (Yedek)
─────────────    ───────    ───────    ───────────────
    %90+         KAPALI     KAPALI         KAPALI
    %70-90       AÇIK       KAPALI         KAPALI
    %50-70       AÇIK       AÇIK           KAPALI
    %50-         AÇIK       AÇIK           AÇIK (Acil)

    Arıza        KAPALI     AÇIK           AÇIK (Devralır)
```

### Kullanılan Function Blocks

| Block | Görev |
|-------|-------|
| **CTU** | Pompa çalışma saatlerini sayar |
| **TON** | Pompa gecikmeli başlatma (surge önleme) |
| **RS Flip-Flop** | Arıza durumu saklar (Reset dominant) |
| **R_TRIG** | Seviye değişim anını tespit eder |

### Arıza Senaryosu

```
1. Pompa 1 arıza verir
   └── PLC arıza sinyali gönderir (Modbus coil)

2. Edge Agent algılar
   ├── RS flip-flop SET olur (arıza kaydedilir)
   ├── Pompa 1 durdurulur
   └── Pompa 3 (yedek) devreye girer

3. Alarm gönderilir
   ├── Cloud'a arıza kaydı
   ├── SMS ile teknisyene bilgi
   └── Webhook ile Slack kanalına mesaj

4. Teknisyen gelir, arızayı giderir
   └── MQTT komutu ile RS flip-flop RESET edilir

5. Normal çalışmaya dönülür
```

---

## Senaryo 5: Soğuk Hava Deposu

### Durum
Bir soğuk hava deposunda -18°C sabit sıcaklık korunmalı, kapı açıldığında alarm verilmeli, kompresör çalışma süresi optimize edilmelidir.

### Sistem

```
┌────────────────────────────────────────────────┐
│              SOĞUK HAVA DEPOSU                 │
│                                                │
│   ┌──────────┐         ┌──────────────────┐   │
│   │ Sıcaklık │         │    Kompresör     │   │
│   │ Sensörü  │         │   (PLC Kontrol)  │   │
│   │  -18°C   │         └────────┬─────────┘   │
│   └────┬─────┘                  │             │
│        │                        │             │
│   ┌────▼────┐              ┌────▼────┐       │
│   │ Modbus  │              │  GPIO   │       │
│   │ (TCP)   │              │ (Röle)  │       │
│   └────┬────┘              └────┬────┘       │
│        │                        │             │
│        └───────────┬────────────┘             │
│                    │                          │
│   ┌────────────────▼────────────────┐        │
│   │        SUDERRA EDGE AGENT        │        │
│   │                                  │        │
│   │  ┌──────────────────────────┐   │        │
│   │  │     HYSTERESIS FB        │   │        │
│   │  │  Set: -17°C  Reset: -19°C│   │        │
│   │  │  (Kompresör kontrolü)    │   │        │
│   │  └──────────────────────────┘   │        │
│   │                                  │        │
│   │  ┌──────────────────────────┐   │        │
│   │  │      TOF Timer           │   │        │
│   │  │  (Kapı açık kalma)       │   │        │
│   │  └──────────────────────────┘   │        │
│   └──────────────────────────────────┘        │
│                                                │
│   [KAPI]  ◄── Manyetik sensör (GPIO input)    │
└────────────────────────────────────────────────┘
```

### Günlük Senaryo

| Saat | Olay | Agent Tepkisi |
|------|------|---------------|
| 08:00 | Depo açılır | Normal izleme başlar |
| 08:15 | Kapı açılır (mal yükleme) | Timer başlar |
| 08:20 | Kapı 5 dk açık | "Kapı açık" uyarısı |
| 08:22 | Kapı kapanır | Timer sıfırlanır |
| 10:30 | Sıcaklık -16°C'ye çıkar | Kompresör çalışır |
| 11:00 | Sıcaklık -19°C'ye düşer | Kompresör durur |
| 14:00 | Elektrik kesilir | Offline çalışma |
| 14:30 | Elektrik gelir | Veriler senkronize edilir |
| 18:00 | Günlük rapor | Cloud'a enerji raporu |

---

## Senaryo 6: Çoklu Lokasyon Yönetimi

### Durum
Bir şirketin 3 farklı şehirde tesisi bulunmaktadır. Her tesiste farklı ekipmanlar izlenmekte, merkezi dashboard'dan yönetilmektedir.

### Yapı

```
                        ┌─────────────────────┐
                        │    CLOUD PLATFORM   │
                        │    (MQTT Broker)    │
                        └──────────┬──────────┘
                                   │
           ┌───────────────────────┼───────────────────────┐
           │                       │                       │
           ▼                       ▼                       ▼
    ┌──────────────┐       ┌──────────────┐       ┌──────────────┐
    │  İSTANBUL    │       │    ANKARA    │       │    İZMİR     │
    │   Fabrika    │       │    Depo      │       │   Çiftlik    │
    │              │       │              │       │              │
    │ ┌──────────┐ │       │ ┌──────────┐ │       │ ┌──────────┐ │
    │ │  Edge    │ │       │ │  Edge    │ │       │ │  Edge    │ │
    │ │  Agent   │ │       │ │  Agent   │ │       │ │  Agent   │ │
    │ └──────────┘ │       │ └──────────┘ │       │ └──────────┘ │
    │              │       │              │       │              │
    │ • 5 PLC      │       │ • 2 PLC      │       │ • 10 Sensör  │
    │ • 20 Sensör  │       │ • 8 Sensör   │       │ • 5 Pompa    │
    │ • 3 Motor    │       │ • 1 Kompresör│       │ • 2 Fan      │
    └──────────────┘       └──────────────┘       └──────────────┘
```

### Her Lokasyon İçin

| Lokasyon | Device ID | Topic Prefix | Özellikler |
|----------|-----------|--------------|------------|
| İstanbul | `ist-fab-001` | `tenants/acme/devices/ist-fab-001/` | Enerji izleme, motor kontrolü |
| Ankara | `ank-dep-001` | `tenants/acme/devices/ank-dep-001/` | Soğutma, stok takibi |
| İzmir | `izm-cft-001` | `tenants/acme/devices/izm-cft-001/` | Su kalitesi, pompa kontrolü |

### Avantajlar

1. **Bağımsız Çalışma**
   - Her edge agent kendi başına çalışır
   - İnternet kesilse bile lokal kontrol devam eder
   - Kritik scriptler her zaman çalışır

2. **Merkezi Yönetim**
   - Tüm lokasyonlar tek dashboard'dan izlenir
   - Script güncellemeleri uzaktan yapılır
   - Alarm kuralları merkezi tanımlanır

3. **Ölçeklenebilirlik**
   - Yeni lokasyon eklemek 5 dakika
   - Mevcut scripter kopyalanabilir
   - Cihaz başına maliyet düşük

---

## Senaryo 7: Güvenlik ve Acil Durum

### Durum
Bir kimya tesisinde gaz sızıntısı, yangın ve su baskını sensörleri izlenmekte, acil durumlarda otomatik önlemler alınmaktadır.

### Acil Durum Hiyerarşisi

```
Öncelik    Durum              Tepki
────────   ─────              ─────
   255     Gaz Sızıntısı      Tüm sistemleri kapat, alarm çal
   200     Yangın             Sprinkler aç, havalandırmayı kapat
   100     Su Baskını         Pompaları çalıştır, alarm ver
    50     Yüksek Sıcaklık    Soğutma aç
```

### Script: Acil Gaz Sızıntısı

```json
{
  "id": "acil-gaz-sizintisi",
  "name": "Acil Durum - Gaz Sızıntısı",
  "priority": "critical",
  "triggers": [
    {
      "trigger_type": "threshold",
      "source": "gaz_seviyesi_ppm",
      "operator": "gt",
      "value": 100
    }
  ],
  "actions": [
    {
      "action_type": "set_gpio",
      "target": "21",
      "value": true,
      "comment": "Acil durum sireni"
    },
    {
      "action_type": "set_gpio",
      "target": "22",
      "value": false,
      "comment": "Tüm motorları kapat"
    },
    {
      "action_type": "write_coil",
      "device": "PLC-Main",
      "address": 100,
      "value": false,
      "comment": "Gaz vanasını kapat"
    },
    {
      "action_type": "webhook",
      "url": "https://api.pagerduty.com/incidents",
      "method": "POST",
      "message": "{\"incident\": {\"title\": \"GAZ SIZINTISI - ${gaz_seviyesi_ppm} ppm\", \"urgency\": \"high\"}}"
    },
    {
      "action_type": "alert",
      "level": "critical",
      "message": "ACİL: Gaz sızıntısı tespit edildi! Seviye: ${gaz_seviyesi_ppm} ppm"
    }
  ]
}
```

### Çatışma Önleme

Birden fazla acil durum aynı anda olursa:

1. **En yüksek öncelikli script kazanır**
   - Gaz sızıntısı (255) > Yangın (200)
   - Çakışan GPIO yazmaları önlenir

2. **Conflict Detection**
   - Aynı GPIO'ya farklı değer yazılmak istenirse
   - Yüksek öncelikli komut uygulanır
   - Düşük öncelikli komut loglanır

---

## Özet: Ne Zaman Hangi Özellik?

| Senaryo | Kullanılacak Özellikler |
|---------|-------------------------|
| Basit izleme | Threshold trigger, Alert action |
| Titreşim önleme | HYSTERESIS function block |
| Zamanlı görevler | Cron trigger, TON/TOF timer |
| Sayaç/Saat takibi | CTU/CTD counter, RETAIN variable |
| Ortalama hesaplama | MAVG function block |
| PID kontrol | PID function block |
| Harici bildirim | Webhook action |
| Acil durumlar | Priority scripts, Critical level |
| Çevrimdışı çalışma | Offline queue (otomatik) |

---

## Başlarken

1. **Cihazı Bağla**
   - Edge agent'ı Raspberry Pi'ye kur
   - Modbus/GPIO bağlantılarını yap

2. **Cloud'a Kaydet**
   - Device code ile aktivasyon yap
   - MQTT bağlantısını kontrol et

3. **İlk Script'i Yaz**
   - Basit bir threshold alarm ile başla
   - Test et ve genişlet

4. **İzle ve İyileştir**
   - Dashboard'dan verileri izle
   - Gerekli script'leri ekle

---

*Suderra Edge Agent v1.2.4 - Endüstriyel IoT için güvenilir çözüm*
