# Suderra Edge Agent - Web API Reference

**Version**: 1.2.6
**Author**: Suderra AS
**Date**: 2026-01-19

---

## Overview

Suderra Edge Agent iki tip API sunar:

1. **HTTP Health API** - Cihaz durumu ve diagnostik bilgileri (yerel erişim)
2. **MQTT Command API** - Bulut platformu ile iletişim (uzak erişim)

---

## 1. HTTP Health API

### Etkinleştirme

```bash
# Build with health feature
cargo build --release --features health
```

### Base URL

```
http://<device-ip>:8080
```

---

### GET /health

Basit sağlık kontrolü - agent çalışıyorsa 200 döner.

**Response:**
```json
{
  "status": "healthy",
  "version": "1.2.6",
  "uptime_secs": 3600
}
```

**Status Değerleri:**
| Status | Açıklama |
|--------|----------|
| `healthy` | Tüm sistemler normal |
| `degraded` | Bazı bileşenler sorunlu |
| `unhealthy` | Kritik hata durumu |

---

### GET /ready

Hazırlık kontrolü - tüm bileşenler hazır mı?

**Response:**
```json
{
  "ready": true,
  "checks": {
    "config_loaded": true,
    "mqtt_connected": true,
    "device_activated": true
  }
}
```

**HTTP Status:**
- `200 OK` - Hazır
- `503 Service Unavailable` - Hazır değil

---

### GET /metrics

Temel metrikler.

**Response:**
```json
{
  "uptime_secs": 86400,
  "mqtt_messages_sent": 1523,
  "mqtt_messages_received": 89,
  "modbus_reads": 45230,
  "script_executions": 12500,
  "offline_queue_size": 0
}
```

---

### GET /diagnostics

Kapsamlı diagnostik bilgileri (troubleshooting için).

**Response:**
```json
{
  "timestamp": "2026-01-19T12:00:00Z",
  "version": "1.2.6",
  "uptime_secs": 86400,
  "system": {
    "os": "Linux 5.15.0",
    "hostname": "edge-device-001",
    "cpu_count": 4,
    "cpu_usage_percent": 25.5,
    "memory_total_bytes": 4294967296,
    "memory_used_bytes": 1073741824,
    "memory_usage_percent": 25.0,
    "disk": {
      "total_bytes": 32212254720,
      "available_bytes": 16106127360,
      "usage_percent": 50.0
    }
  },
  "process": {
    "pid": 1234,
    "memory_bytes": 52428800,
    "thread_count": 8,
    "start_time": "2026-01-18T12:00:00Z"
  },
  "components": {
    "mqtt": {
      "connected": true,
      "messages_sent": 1523,
      "messages_received": 89,
      "last_connected": "2026-01-19T11:55:00Z"
    },
    "modbus": {
      "client_count": 3,
      "total_reads": 45230,
      "read_errors": 12,
      "circuit_states": [
        ["inverter1", "closed"],
        ["plc1", "closed"],
        ["meter1", "half_open"]
      ]
    },
    "scripts": {
      "loaded_count": 5,
      "active_count": 3,
      "total_executions": 12500,
      "execution_errors": 2
    },
    "function_blocks": {
      "instance_count": 12,
      "type_counts": {
        "TON": 4,
        "PID": 2,
        "SR": 3,
        "CTU": 3
      }
    },
    "offline_queue": {
      "size": 0,
      "capacity": 10000,
      "total_queued": 156,
      "total_sent": 156
    }
  },
  "config": {
    "device_id": "dev_****1234",
    "mqtt_host": "mqtt.suderra.com",
    "modbus_device_count": 3,
    "gpio_mapping_count": 8,
    "telemetry_interval_secs": 30
  },
  "recent_errors": [
    "[2026-01-19T11:50:00Z] Modbus read timeout on inverter1",
    "[2026-01-19T11:45:00Z] Circuit breaker opened for meter1"
  ]
}
```

---

## 2. MQTT Command API

### Topic Yapısı

```
devices/{device_id}/commands    # Gelen komutlar (subscribe)
devices/{device_id}/responses   # Komut yanıtları (publish)
devices/{device_id}/telemetry   # Telemetri verileri (publish)
devices/{device_id}/status      # Cihaz durumu (publish)
devices/{device_id}/config      # Konfig güncellemeleri (subscribe)
```

---

### Command Message Format

**Request:**
```json
{
  "command_id": "cmd_unique_id_123",
  "command": "ping",
  "params": {},
  "timestamp": "2026-01-19T12:00:00Z"
}
```

**Response:**
```json
{
  "command_id": "cmd_unique_id_123",
  "device_id": "device_001",
  "success": true,
  "result": { "pong": true },
  "timestamp": "2026-01-19T12:00:01Z",
  "error": null
}
```

---

### Mevcut Komutlar

#### System Commands

| Command | Açıklama | Params |
|---------|----------|--------|
| `ping` | Bağlantı testi | - |
| `get_info` | Cihaz bilgileri | - |
| `get_config` | Aktif konfigürasyon | - |
| `get_hardware` | Donanım durumu | - |
| `reboot` | Sistemi yeniden başlat | `delay_seconds` (optional) |
| `restart_agent` | Agent'ı yeniden başlat | - |
| `set_log_level` | Log seviyesi değiştir | `level` |

#### Modbus Commands

| Command | Açıklama | Params |
|---------|----------|--------|
| `read_modbus` | Register oku | `device`, `address`, `count` |
| `write_modbus` | Register yaz | `device`, `address`, `value` |

#### GPIO Commands

| Command | Açıklama | Params |
|---------|----------|--------|
| `read_gpio` | Tüm pin durumları | - |
| `write_gpio` | Pin değeri yaz | `pin`, `value` |

#### Script Commands

| Command | Açıklama | Params |
|---------|----------|--------|
| `list_scripts` | Script listesi | - |
| `get_script` | Script detayı | `script_id` |
| `deploy_script` | Script yükle | `script` (object) |
| `delete_script` | Script sil | `script_id` |
| `enable_script` | Script etkinleştir | `script_id` |
| `disable_script` | Script devre dışı | `script_id` |

#### IEC 61131-3 Program Commands

| Command | Açıklama | Params |
|---------|----------|--------|
| `deploy_program` | Program yükle | `program` (object) |
| `get_program` | Aktif program | - |
| `rollback_program` | Önceki versiyona dön | - |

---

### Komut Örnekleri

#### ping

```json
// Request
{
  "command_id": "cmd_001",
  "command": "ping",
  "params": {}
}

// Response
{
  "command_id": "cmd_001",
  "device_id": "device_001",
  "success": true,
  "result": {
    "pong": true,
    "timestamp": "2026-01-19T12:00:00Z"
  },
  "error": null
}
```

#### read_modbus

```json
// Request
{
  "command_id": "cmd_002",
  "command": "read_modbus",
  "params": {
    "device": "inverter1",
    "address": 100,
    "count": 2
  }
}

// Response
{
  "command_id": "cmd_002",
  "device_id": "device_001",
  "success": true,
  "result": {
    "device": "inverter1",
    "values": [
      { "address": 100, "value": 2305, "scaled": 230.5 },
      { "address": 101, "value": 500, "scaled": 50.0 }
    ]
  },
  "error": null
}
```

#### write_gpio

```json
// Request
{
  "command_id": "cmd_003",
  "command": "write_gpio",
  "params": {
    "pin": 17,
    "value": true
  }
}

// Response
{
  "command_id": "cmd_003",
  "device_id": "device_001",
  "success": true,
  "result": {
    "pin": 17,
    "value": true,
    "previous": false
  },
  "error": null
}
```

#### deploy_script

```json
// Request
{
  "command_id": "cmd_004",
  "command": "deploy_script",
  "params": {
    "script": {
      "id": "alarm_handler",
      "name": "Alarm Handler",
      "enabled": true,
      "triggers": [
        {
          "type": "threshold",
          "source": "sensor:temperature",
          "operator": "gt",
          "value": 80
        }
      ],
      "actions": [
        {
          "type": "gpio",
          "pin": 18,
          "value": true
        },
        {
          "type": "webhook",
          "url": "https://api.example.com/alarm",
          "method": "POST"
        }
      ]
    }
  }
}

// Response
{
  "command_id": "cmd_004",
  "device_id": "device_001",
  "success": true,
  "result": {
    "script_id": "alarm_handler",
    "status": "deployed"
  },
  "error": null
}
```

---

## 3. Telemetry Message Format

Agent periyodik olarak telemetri verileri gönderir.

```json
{
  "device_id": "device_001",
  "device_code": "SUDERRA-001",
  "timestamp": "2026-01-19T12:00:00Z",
  "metrics": {
    "cpu_usage_percent": 25.5,
    "memory_usage_percent": 45.2,
    "memory_used_mb": 1843,
    "memory_total_mb": 4096,
    "disk_usage_percent": 62.3,
    "disk_used_gb": 19.92,
    "disk_total_gb": 32.0,
    "temperature_celsius": 55.0,
    "network_rx_bytes": 123456789,
    "network_tx_bytes": 987654321,
    "modbus": [
      {
        "device_name": "inverter1",
        "registers": [
          { "name": "voltage", "value": 230.5, "unit": "V" },
          { "name": "current", "value": 15.2, "unit": "A" },
          { "name": "power", "value": 3503.6, "unit": "W" }
        ],
        "errors": []
      }
    ],
    "gpio": [
      { "pin": 17, "name": "relay1", "state": "high", "direction": "output" },
      { "pin": 18, "name": "sensor1", "state": "low", "direction": "input" }
    ]
  }
}
```

---

## 4. Status Message Format

Cihaz durumu değişikliklerinde gönderilir.

```json
{
  "device_id": "device_001",
  "device_code": "SUDERRA-001",
  "status": "online",
  "timestamp": "2026-01-19T12:00:00Z",
  "agent_version": "1.2.6",
  "uptime_seconds": 86400
}
```

**Status Değerleri:**
| Status | Açıklama |
|--------|----------|
| `online` | Normal çalışma |
| `offline` | Bağlantı kesik (Last Will) |
| `maintenance` | Bakım modu |
| `error` | Hata durumu |

---

## 5. Error Codes

| Code | Açıklama |
|------|----------|
| `INVALID_PARAMS` | Geçersiz parametreler |
| `NOT_FOUND` | Kaynak bulunamadı |
| `TIMEOUT` | İşlem zaman aşımı |
| `NOT_CONNECTED` | Bağlantı yok |
| `PERMISSION_DENIED` | Yetki hatası |
| `RATE_LIMITED` | Rate limit aşıldı |
| `CIRCUIT_OPEN` | Circuit breaker açık |

---

## 6. Rate Limiting

| Endpoint | Limit |
|----------|-------|
| MQTT Commands | 60/dakika |
| Modbus Writes | 10/saniye |
| Telemetry Publish | Configurable (default 30s) |

---

## 7. Security

### TLS/mTLS

```yaml
mqtt:
  host: "mqtt.suderra.com"
  port: 8883
  tls:
    enabled: true
    ca_cert: "/etc/suderra/ca.crt"
    client_cert: "/etc/suderra/client.crt"
    client_key: "/etc/suderra/client.key"
```

### Authentication

- **MQTT**: Username/Password veya mTLS
- **HTTP Health**: Yerel ağdan erişim (firewall ile korunmalı)

---

## 8. OpenTelemetry Integration

```yaml
telemetry:
  otlp:
    endpoint: "http://jaeger:4317"
    service_name: "suderra-agent"
    sample_ratio: 1.0
```

Trace'ler:
- MQTT message handling
- Command execution
- Modbus operations
- Script execution

---

## 9. Logging

### Log Seviyeleri

```bash
# Environment variable ile
RUST_LOG=debug suderra-agent

# Komut ile runtime'da
{
  "command": "set_log_level",
  "params": { "level": "debug" }
}
```

| Level | Açıklama |
|-------|----------|
| `error` | Sadece hatalar |
| `warn` | Uyarılar ve hatalar |
| `info` | Genel bilgiler (varsayılan) |
| `debug` | Detaylı debug bilgileri |
| `trace` | Tüm detaylar |

### Log Formatı (v1.2.6)

```log
2026-01-19T12:00:00Z INFO  📥 MQTT message received: topic='devices/001/commands', size=256 bytes, qos=AtLeastOnce
2026-01-19T12:00:00Z INFO  ⚡ Command received: id='cmd_001', command='ping', has_params=false
2026-01-19T12:00:01Z INFO  ✅ Command completed: id='cmd_001', command='ping', success=true, duration=1.234ms
2026-01-19T12:00:01Z INFO  📤 MQTT response published: topic='devices/001/responses', command_id='cmd_001', success=true
```

---

*Generated by Suderra AS*
