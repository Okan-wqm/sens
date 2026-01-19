# Edge Scenarios: What SCADA Cannot Do

This document describes use cases where the Edge Agent provides capabilities that traditional SCADA/PLC systems cannot achieve. These scenarios focus on cloud connectivity, external data integration, and intelligent decision-making.

---

## Why Edge Complements SCADA

```
┌─────────────────────────────────────────────────────────────────────┐
│                    CAPABILITY COMPARISON                             │
├─────────────────────────────────┬───────────────────────────────────┤
│         SCADA/PLC CAN DO        │       SCADA/PLC CANNOT DO         │
├─────────────────────────────────┼───────────────────────────────────┤
│ ✓ PID control                   │ ✗ Fetch weather forecast API      │
│ ✓ A-B solution mixing           │ ✗ Send Slack/Teams notifications  │
│ ✓ EC/pH dosing                  │ ✗ Query electricity spot prices   │
│ ✓ Timer-based automation        │ ✗ Compare data across facilities  │
│ ✓ Alarm contacts                │ ✗ Generate compliance reports     │
│ ✓ HMI display                   │ ✗ Predict harvest dates           │
│ ✓ Basic data logging            │ ✗ Adjust recipes from cloud       │
│ ✓ Modbus communication          │ ✗ Integrate with ERP/MES          │
└─────────────────────────────────┴───────────────────────────────────┘
```

**Key Insight**: SCADA excels at deterministic, real-time control. Edge excels at connectivity, external data, and intelligent orchestration.

---

## Scenario 1: Weather-Adaptive Feeding Schedule

### The Problem SCADA Cannot Solve

Fish feeding is typically time-based (e.g., 4 times daily at fixed hours). But fish appetite varies with:
- Barometric pressure changes (fish stop eating before storms)
- Water temperature trends (not just current value)
- Dissolved oxygen forecasts

SCADA has no way to fetch tomorrow's weather forecast or adjust feeding based on atmospheric pressure trends.

### Edge Solution

```
┌────────────────────────────────────────────────────────────────────┐
│                  WEATHER-ADAPTIVE FEEDING                           │
│                                                                     │
│   EXTERNAL APIs              EDGE AGENT              PLC           │
│                                                                     │
│   ┌─────────────┐        ┌──────────────┐      ┌──────────────┐   │
│   │ OpenWeather │───────▶│              │      │              │   │
│   │ API         │  JSON  │  Correlates: │      │   FEEDING    │   │
│   └─────────────┘        │  • Pressure  │      │   SYSTEM     │   │
│                          │  • Temp trend│ Modbus│              │   │
│   ┌─────────────┐        │  • DO level  │─────▶│  Receives:   │   │
│   │ Tide Tables │───────▶│  • Tide      │      │  • Tank ID   │   │
│   │ API         │        │              │      │  • Amount g  │   │
│   └─────────────┘        │  Calculates: │      │  • Start cmd │   │
│                          │  Feed amount │      │              │   │
│   ┌─────────────┐        │  adjustment  │      └──────────────┘   │
│   │ Historical  │───────▶│  (-30% to    │                         │
│   │ Feed Data   │        │   +10%)      │                         │
│   └─────────────┘        └──────────────┘                         │
│                                                                     │
└────────────────────────────────────────────────────────────────────┘
```

### Script Example

```json
{
  "id": "weather-adaptive-feeding",
  "name": "Weather-Adjusted Feed Calculator",
  "triggers": [
    {
      "trigger_type": "cron",
      "cron": "0 5 * * *",
      "comment": "Run at 5 AM to plan the day"
    }
  ],
  "actions": [
    {
      "action_type": "webhook",
      "url": "https://api.openweathermap.org/data/2.5/forecast?lat=41.0&lon=29.0&appid=${env:OPENWEATHER_KEY}",
      "method": "GET",
      "store_response": "weather_data"
    },
    {
      "action_type": "set_variable",
      "target": "pressure_trend",
      "value": "${json:weather_data:list[0].main.pressure} - ${json:weather_data:list[4].main.pressure}",
      "scope": "local",
      "comment": "Pressure change over next 12 hours"
    },
    {
      "action_type": "set_variable",
      "target": "feed_modifier",
      "value": "${calc:if(${var:pressure_trend} < -5, 0.7, if(${var:pressure_trend} < -2, 0.85, 1.0))}",
      "scope": "global",
      "comment": "Reduce feed 30% if pressure dropping fast (storm coming)"
    },
    {
      "action_type": "publish_mqtt",
      "target": "feeding/daily-plan",
      "message": "{\"date\":\"${time:date}\",\"modifier\":${var:feed_modifier},\"reason\":\"pressure_trend=${var:pressure_trend}\"}"
    },
    {
      "action_type": "alert",
      "level": "info",
      "message": "Today's feed modifier: ${var:feed_modifier}x (pressure trend: ${var:pressure_trend} hPa)",
      "condition": {
        "sensor": "${var:feed_modifier}",
        "operator": "less_than",
        "value": 1.0
      }
    }
  ]
}
```

### Why This Matters

| Metric | Fixed Schedule | Weather-Adaptive |
|--------|---------------|------------------|
| Feed waste | 15-20% | 5-8% |
| FCR (Feed Conversion Ratio) | 1.6 | 1.3 |
| Annual feed cost (100 ton production) | $180,000 | $145,000 |

---

## Scenario 2: Electricity Spot Price Optimization

### The Problem

Microalgae photobioreactors, RAS systems, and greenhouses consume significant electricity. Energy costs vary by:
- Time of day (peak vs. off-peak)
- Real-time spot market prices
- Renewable energy availability

SCADA cannot query electricity market APIs or make economic decisions.

### Edge Solution

```
┌────────────────────────────────────────────────────────────────────┐
│               ENERGY COST OPTIMIZATION                              │
│                                                                     │
│   ┌─────────────┐                                                  │
│   │ Spot Price  │                                                  │
│   │ API (EPIAS) │──┐                                               │
│   └─────────────┘  │      ┌──────────────┐      ┌──────────────┐  │
│                    │      │              │      │              │  │
│   ┌─────────────┐  ├─────▶│    EDGE      │─────▶│     PLC      │  │
│   │ Solar Panel │  │      │              │      │              │  │
│   │ Production  │──┤      │  Decides:    │      │  Executes:   │  │
│   └─────────────┘  │      │  • Run now?  │      │  • Pump on   │  │
│                    │      │  • Wait 2h?  │      │  • Chiller   │  │
│   ┌─────────────┐  │      │  • Reduce?   │      │  • Lights    │  │
│   │ Production  │──┘      │              │      │              │  │
│   │ Schedule    │         └──────────────┘      └──────────────┘  │
│   └─────────────┘                                                  │
│                                                                     │
│   DECISION MATRIX:                                                 │
│   ┌────────────────────────────────────────────────────────────┐  │
│   │ Price < 0.05 $/kWh  → Run all deferrable loads            │  │
│   │ Price 0.05-0.10     → Normal operation                     │  │
│   │ Price 0.10-0.15     → Defer non-critical (lighting, etc.) │  │
│   │ Price > 0.15        → Emergency loads only                 │  │
│   └────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────┘
```

### Script Example

```json
{
  "id": "energy-optimizer",
  "name": "Spot Price Energy Manager",
  "triggers": [
    {
      "trigger_type": "interval",
      "interval_seconds": 900,
      "comment": "Check every 15 minutes"
    }
  ],
  "actions": [
    {
      "action_type": "webhook",
      "url": "https://seffaflik.epias.com.tr/electricity-service/v1/markets/dam/data/mcp",
      "method": "GET",
      "store_response": "price_data"
    },
    {
      "action_type": "set_variable",
      "target": "current_price",
      "value": "${json:price_data:items[0].price}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "price_tier",
      "value": "${calc:if(${var:current_price} < 50, 1, if(${var:current_price} < 100, 2, if(${var:current_price} < 150, 3, 4)))}",
      "scope": "global"
    },
    {
      "action_type": "write_modbus",
      "device": "PLC-Main",
      "address": 700,
      "value": "${var:price_tier}",
      "comment": "PLC uses tier for load shedding decisions"
    },
    {
      "action_type": "write_modbus",
      "device": "PLC-Lighting",
      "address": 100,
      "value": "${calc:if(${var:price_tier} >= 3, 0, 100)}",
      "comment": "Turn off supplemental lighting if price tier 3+"
    },
    {
      "action_type": "publish_mqtt",
      "target": "energy/spot-price",
      "message": "{\"price\":${var:current_price},\"tier\":${var:price_tier},\"time\":\"${time:iso}\"}"
    }
  ]
}
```

### Savings Example

| Month | Fixed Rate Cost | Spot-Optimized Cost | Savings |
|-------|----------------|--------------------:|--------:|
| January | $12,400 | $9,800 | 21% |
| July | $18,200 | $13,100 | 28% |
| Annual | $156,000 | $118,000 | **$38,000** |

---

## Scenario 3: Multi-Site Production Comparison

### The Problem

A company operates 5 microalgae facilities. Management needs to:
- Compare productivity across sites
- Identify underperforming reactors
- Share best practices automatically

SCADA systems are isolated. They cannot communicate with other facilities or aggregate data.

### Edge Solution

```
┌────────────────────────────────────────────────────────────────────┐
│                MULTI-SITE INTELLIGENCE                              │
│                                                                     │
│   SITE A          SITE B          SITE C          CLOUD            │
│   ┌─────┐         ┌─────┐         ┌─────┐         ┌─────────┐      │
│   │Edge │         │Edge │         │Edge │         │ Central │      │
│   │Agent│         │Agent│         │Agent│         │ Server  │      │
│   └──┬──┘         └──┬──┘         └──┬──┘         └────┬────┘      │
│      │               │               │                  │          │
│      └───────────────┴───────────────┴────── MQTT ─────┘          │
│                                                                     │
│   CLOUD AGGREGATES:                                                │
│   ┌────────────────────────────────────────────────────────────┐  │
│   │ Site    │ Reactor │ OD/day │ Yield g/L │ Status           │  │
│   │─────────│─────────│────────│───────────│──────────────────│  │
│   │ Site-A  │ PBR-01  │ 0.42   │ 2.8       │ ✓ Normal         │  │
│   │ Site-A  │ PBR-02  │ 0.38   │ 2.5       │ ✓ Normal         │  │
│   │ Site-B  │ PBR-01  │ 0.21   │ 1.4       │ ⚠ UNDERPERFORM  │  │
│   │ Site-B  │ PBR-02  │ 0.40   │ 2.7       │ ✓ Normal         │  │
│   │ Site-C  │ PBR-01  │ 0.45   │ 3.0       │ ★ Best           │  │
│   └────────────────────────────────────────────────────────────┘  │
│                                                                     │
│   EDGE RECEIVES FROM CLOUD:                                        │
│   • Benchmark: "Site-C PBR-01 achieving 0.45 OD/day"              │
│   • Alert: "Your PBR-02 is 23% below network average"             │
│   • Recipe: "Site-C using CO2 pulse every 4h, try it"             │
└────────────────────────────────────────────────────────────────────┘
```

### Local Edge Script (runs on each site)

```json
{
  "id": "site-reporter",
  "name": "Daily Production Report to Cloud",
  "triggers": [
    {
      "trigger_type": "cron",
      "cron": "0 23 * * *",
      "comment": "End of day report"
    }
  ],
  "actions": [
    {
      "action_type": "set_variable",
      "target": "pbr1_od",
      "value": "${modbus:Turbidity-PBR1:od_value}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "pbr1_od_yesterday",
      "value": "${var:retain:pbr1_yesterday}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "pbr1_growth_rate",
      "value": "${calc:${var:pbr1_od} - ${var:pbr1_od_yesterday}}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "pbr1_yesterday",
      "value": "${var:pbr1_od}",
      "scope": "retain"
    },
    {
      "action_type": "publish_mqtt",
      "target": "sites/${device_id}/daily-production",
      "message": "{\"reactor\":\"PBR-01\",\"od\":${var:pbr1_od},\"growth_rate\":${var:pbr1_growth_rate},\"co2_used\":${modbus:CO2-Meter:total_kg},\"energy_kwh\":${modbus:Energy-Meter:daily_kwh}}"
    },
    {
      "action_type": "webhook",
      "url": "https://api.company.com/production/report",
      "method": "POST",
      "message": "{\"site\":\"${config:site_name}\",\"date\":\"${time:date}\",\"reactors\":[{\"id\":\"PBR-01\",\"od\":${var:pbr1_od},\"rate\":${var:pbr1_growth_rate}}]}"
    }
  ]
}
```

### Cloud-Pushed Recipe Update

```json
{
  "id": "cloud-recipe-receiver",
  "name": "Apply Cloud Recipe Updates",
  "triggers": [
    {
      "trigger_type": "mqtt",
      "topic": "sites/${device_id}/recipe-update"
    }
  ],
  "actions": [
    {
      "action_type": "set_variable",
      "target": "new_co2_interval",
      "value": "${mqtt:payload:co2_interval_minutes}",
      "scope": "global"
    },
    {
      "action_type": "set_variable",
      "target": "new_light_intensity",
      "value": "${mqtt:payload:light_percent}",
      "scope": "global"
    },
    {
      "action_type": "write_modbus",
      "device": "PLC-Bioreactor",
      "address": 500,
      "value": "${var:new_co2_interval}"
    },
    {
      "action_type": "write_modbus",
      "device": "PLC-Lighting",
      "address": 100,
      "value": "${var:new_light_intensity}"
    },
    {
      "action_type": "alert",
      "level": "info",
      "message": "Recipe updated from cloud: CO2 every ${var:new_co2_interval}min, light ${var:new_light_intensity}%"
    }
  ]
}
```

---

## Scenario 4: Automated Compliance Reporting

### The Problem

Aquaculture and food production facilities must submit regulatory reports:
- Daily water quality logs
- Chemical usage records
- Temperature excursion reports
- Traceability data

SCADA can log data locally, but cannot generate formatted reports or submit to government portals.

### Edge Solution

```
┌────────────────────────────────────────────────────────────────────┐
│                AUTOMATED COMPLIANCE REPORTING                       │
│                                                                     │
│   DATA COLLECTION          REPORT GENERATION        SUBMISSION     │
│                                                                     │
│   ┌─────────────┐         ┌──────────────┐        ┌────────────┐  │
│   │ pH readings │         │              │        │ Government │  │
│   │ (every 5min)│────────▶│    EDGE      │───────▶│ Portal API │  │
│   └─────────────┘         │              │        └────────────┘  │
│                           │  Aggregates: │                         │
│   ┌─────────────┐         │  • Daily min │        ┌────────────┐  │
│   │ Temperature │────────▶│  • Daily max │───────▶│ Company    │  │
│   │ (every 1min)│         │  • Averages  │        │ ERP System │  │
│   └─────────────┘         │  • Excursions│        └────────────┘  │
│                           │              │                         │
│   ┌─────────────┐         │  Formats:    │        ┌────────────┐  │
│   │ Chemical    │────────▶│  • JSON      │───────▶│ Auditor    │  │
│   │ dosing logs │         │  • CSV       │        │ Email      │  │
│   └─────────────┘         │  • PDF       │        └────────────┘  │
│                           └──────────────┘                         │
│                                                                     │
└────────────────────────────────────────────────────────────────────┘
```

### Script Example

```json
{
  "id": "daily-compliance-report",
  "name": "Generate and Submit Daily Compliance Report",
  "triggers": [
    {
      "trigger_type": "cron",
      "cron": "0 6 * * *",
      "comment": "Generate at 6 AM for previous day"
    }
  ],
  "actions": [
    {
      "action_type": "set_variable",
      "target": "report_date",
      "value": "${time:yesterday}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "ph_min",
      "value": "${db:query:SELECT MIN(value) FROM readings WHERE sensor='ph' AND date='${var:report_date}'}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "ph_max",
      "value": "${db:query:SELECT MAX(value) FROM readings WHERE sensor='ph' AND date='${var:report_date}'}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "ph_avg",
      "value": "${db:query:SELECT AVG(value) FROM readings WHERE sensor='ph' AND date='${var:report_date}'}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "temp_excursions",
      "value": "${db:query:SELECT COUNT(*) FROM readings WHERE sensor='temp' AND (value < 18 OR value > 24) AND date='${var:report_date}'}",
      "scope": "local"
    },
    {
      "action_type": "webhook",
      "url": "https://regulatory.gov.tr/api/v1/aquaculture/daily-report",
      "method": "POST",
      "message": "{\"facility_id\":\"${config:facility_license}\",\"date\":\"${var:report_date}\",\"water_quality\":{\"ph\":{\"min\":${var:ph_min},\"max\":${var:ph_max},\"avg\":${var:ph_avg}},\"temp_excursions\":${var:temp_excursions}},\"certification\":\"auto-generated\"}"
    },
    {
      "action_type": "webhook",
      "url": "https://erp.company.com/api/production/water-quality",
      "method": "POST",
      "message": "{\"site\":\"${config:site_name}\",\"date\":\"${var:report_date}\",\"ph_avg\":${var:ph_avg}}"
    },
    {
      "action_type": "alert",
      "level": "warning",
      "message": "Compliance alert: ${var:temp_excursions} temperature excursions yesterday",
      "condition": {
        "sensor": "${var:temp_excursions}",
        "operator": "greater_than",
        "value": 0
      }
    }
  ]
}
```

---

## Scenario 5: Predictive Harvest Timing

### The Problem

Harvest timing affects product quality and price:
- Harvest too early: Lower yield
- Harvest too late: Quality degradation, higher costs
- Market timing: Prices vary by day/week

SCADA sees only current state. It cannot analyze growth trends or predict optimal harvest date.

### Edge Solution

```
┌────────────────────────────────────────────────────────────────────┐
│                  PREDICTIVE HARVEST OPTIMIZATION                    │
│                                                                     │
│   GROWTH DATA              PREDICTION MODEL         OUTPUT         │
│                                                                     │
│   Historical               ┌──────────────┐                        │
│   ┌─────────┐              │              │        ┌────────────┐  │
│   │ 30-day  │─────────────▶│  Calculate:  │───────▶│ Optimal    │  │
│   │ OD trend│              │              │        │ harvest in │  │
│   └─────────┘              │  • Growth    │        │ 4.2 days   │  │
│                            │    rate/day  │        └────────────┘  │
│   Current                  │              │                        │
│   ┌─────────┐              │  • Days to   │        ┌────────────┐  │
│   │ OD now  │─────────────▶│    target OD │───────▶│ Expected   │  │
│   │ = 2.1   │              │              │        │ yield:     │  │
│   └─────────┘              │  • Yield     │        │ 847 kg     │  │
│                            │    estimate  │        └────────────┘  │
│   Target                   │              │                        │
│   ┌─────────┐              │  • Market    │        ┌────────────┐  │
│   │ OD 2.8  │─────────────▶│    price     │───────▶│ Best price │  │
│   │         │              │    forecast  │        │ Thursday   │  │
│   └─────────┘              └──────────────┘        └────────────┘  │
│                                                                     │
│   FORMULA:                                                         │
│   Days to harvest = (Target OD - Current OD) / Daily growth rate   │
│   Expected yield = Volume × (Target OD × conversion factor)        │
└────────────────────────────────────────────────────────────────────┘
```

### Script Example

```json
{
  "id": "harvest-predictor",
  "name": "Harvest Date and Yield Predictor",
  "triggers": [
    {
      "trigger_type": "interval",
      "interval_seconds": 21600,
      "comment": "Update every 6 hours"
    }
  ],
  "actions": [
    {
      "action_type": "set_variable",
      "target": "current_od",
      "value": "${modbus:Turbidity-PBR1:od_value}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "target_od",
      "value": "2.8",
      "scope": "global"
    },
    {
      "action_type": "set_variable",
      "target": "od_7days_ago",
      "value": "${var:retain:od_history_7d}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "daily_growth",
      "value": "${calc:(${var:current_od} - ${var:od_7days_ago}) / 7}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "days_to_harvest",
      "value": "${calc:(${var:target_od} - ${var:current_od}) / ${var:daily_growth}}",
      "scope": "global"
    },
    {
      "action_type": "set_variable",
      "target": "harvest_date",
      "value": "${calc:date_add(now(), ${var:days_to_harvest})}",
      "scope": "global"
    },
    {
      "action_type": "set_variable",
      "target": "expected_yield_kg",
      "value": "${calc:${config:reactor_volume_l} * ${var:target_od} * 0.85 / 1000}",
      "scope": "local"
    },
    {
      "action_type": "publish_mqtt",
      "target": "production/harvest-forecast",
      "message": "{\"reactor\":\"PBR-01\",\"current_od\":${var:current_od},\"daily_growth\":${var:daily_growth},\"days_remaining\":${var:days_to_harvest},\"expected_yield_kg\":${var:expected_yield_kg},\"estimated_date\":\"${var:harvest_date}\"}"
    },
    {
      "action_type": "webhook",
      "url": "https://api.company.com/planning/harvest-forecast",
      "method": "POST",
      "message": "{\"reactor\":\"PBR-01\",\"date\":\"${var:harvest_date}\",\"yield\":${var:expected_yield_kg}}"
    },
    {
      "action_type": "alert",
      "level": "info",
      "message": "Harvest forecast: ${var:days_to_harvest} days, yield ${var:expected_yield_kg}kg",
      "condition": {
        "sensor": "${var:days_to_harvest}",
        "operator": "less_than",
        "value": 3
      }
    }
  ]
}
```

---

## Scenario 6: Instant Team Notifications

### The Problem

When critical alarms occur:
- SCADA shows alarm on HMI (operator must be watching)
- SCADA sounds buzzer (operator must be nearby)
- Night shift? Weekend? Unmanned facility?

SCADA cannot send Slack messages, SMS, or call on-duty personnel.

### Edge Solution

```
┌────────────────────────────────────────────────────────────────────┐
│                MULTI-CHANNEL ALERT SYSTEM                           │
│                                                                     │
│   ALARM SOURCE           EDGE ROUTING           DESTINATIONS       │
│                                                                     │
│   ┌─────────────┐        ┌──────────────┐      ┌──────────────┐   │
│   │ pH < 6.5    │───────▶│              │─────▶│ Slack        │   │
│   │ CRITICAL    │        │   Priority   │      │ #alerts      │   │
│   └─────────────┘        │   Router     │      └──────────────┘   │
│                          │              │                          │
│   ┌─────────────┐        │  CRITICAL:   │      ┌──────────────┐   │
│   │ Pump fail   │───────▶│  → All       │─────▶│ PagerDuty    │   │
│   │ CRITICAL    │        │    channels  │      │ On-call      │   │
│   └─────────────┘        │              │      └──────────────┘   │
│                          │  WARNING:    │                          │
│   ┌─────────────┐        │  → Slack     │      ┌──────────────┐   │
│   │ Temp drift  │───────▶│  → Log       │─────▶│ SMS Gateway  │   │
│   │ WARNING     │        │              │      │ Manager      │   │
│   └─────────────┘        │  INFO:       │      └──────────────┘   │
│                          │  → Log only  │                          │
│   ┌─────────────┐        │              │      ┌──────────────┐   │
│   │ Daily stats │───────▶│              │─────▶│ Teams        │   │
│   │ INFO        │        └──────────────┘      │ Webhook      │   │
│   └─────────────┘                              └──────────────┘   │
│                                                                     │
└────────────────────────────────────────────────────────────────────┘
```

### Script Example

```json
{
  "id": "critical-alert-dispatcher",
  "name": "Multi-Channel Critical Alert",
  "triggers": [
    {
      "trigger_type": "sensor",
      "sensor": "ph_sensor",
      "operator": "less_than",
      "value": 6.5
    }
  ],
  "actions": [
    {
      "action_type": "webhook",
      "url": "https://hooks.slack.com/services/XXX/YYY/ZZZ",
      "method": "POST",
      "message": "{\"text\":\"🚨 CRITICAL: pH dropped to ${modbus:PLC-WQ:ph_sensor}! Tank: ${config:tank_name}\",\"channel\":\"#facility-alerts\"}"
    },
    {
      "action_type": "webhook",
      "url": "https://events.pagerduty.com/v2/enqueue",
      "method": "POST",
      "message": "{\"routing_key\":\"${env:PAGERDUTY_KEY}\",\"event_action\":\"trigger\",\"payload\":{\"summary\":\"Critical pH alarm at ${config:site_name}\",\"severity\":\"critical\",\"source\":\"${device_id}\"}}"
    },
    {
      "action_type": "webhook",
      "url": "https://api.twilio.com/2010-04-01/Accounts/${env:TWILIO_SID}/Messages.json",
      "method": "POST",
      "message": "To=${config:manager_phone}&From=${env:TWILIO_NUMBER}&Body=CRITICAL: pH ${modbus:PLC-WQ:ph_sensor} at ${config:site_name}"
    },
    {
      "action_type": "publish_mqtt",
      "target": "alarms/critical",
      "message": "{\"type\":\"ph_low\",\"value\":${modbus:PLC-WQ:ph_sensor},\"site\":\"${config:site_name}\",\"time\":\"${time:iso}\"}"
    }
  ]
}
```

---

## Scenario 7: Supply Chain Integration

### The Problem

Production planning requires:
- Feed/nutrient stock levels
- Supplier delivery schedules
- Production forecasts

SCADA cannot query ERP systems or trigger purchase orders.

### Edge Solution

```
┌────────────────────────────────────────────────────────────────────┐
│                  SUPPLY CHAIN INTEGRATION                           │
│                                                                     │
│   LOCAL SENSORS           EDGE LOGIC              ERP/SUPPLIERS    │
│                                                                     │
│   ┌─────────────┐        ┌──────────────┐       ┌──────────────┐  │
│   │ Feed silo   │───────▶│              │──────▶│ SAP Purchase │  │
│   │ level: 15%  │        │  IF level    │       │ Requisition  │  │
│   └─────────────┘        │  < 20%       │       └──────────────┘  │
│                          │              │                          │
│   ┌─────────────┐        │  AND next    │       ┌──────────────┐  │
│   │ Production  │───────▶│  delivery    │──────▶│ Supplier     │  │
│   │ forecast    │        │  > 3 days    │       │ Portal API   │  │
│   └─────────────┘        │              │       └──────────────┘  │
│                          │  THEN create │                          │
│   ┌─────────────┐        │  urgent PO   │       ┌──────────────┐  │
│   │ Consumption │───────▶│              │──────▶│ Logistics    │  │
│   │ rate        │        └──────────────┘       │ Coordinator  │  │
│   └─────────────┘                               └──────────────┘  │
│                                                                     │
└────────────────────────────────────────────────────────────────────┘
```

### Script Example

```json
{
  "id": "feed-inventory-manager",
  "name": "Automatic Feed Reorder",
  "triggers": [
    {
      "trigger_type": "interval",
      "interval_seconds": 3600,
      "comment": "Check hourly"
    }
  ],
  "actions": [
    {
      "action_type": "set_variable",
      "target": "silo_level",
      "value": "${modbus:Silo-Sensor:level_percent}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "daily_consumption",
      "value": "${var:retain:avg_daily_feed_kg}",
      "scope": "local"
    },
    {
      "action_type": "set_variable",
      "target": "days_remaining",
      "value": "${calc:(${config:silo_capacity_kg} * ${var:silo_level} / 100) / ${var:daily_consumption}}",
      "scope": "local"
    },
    {
      "action_type": "webhook",
      "url": "https://erp.company.com/api/inventory/feed-status",
      "method": "POST",
      "message": "{\"site\":\"${config:site_name}\",\"level_pct\":${var:silo_level},\"days_remaining\":${var:days_remaining},\"daily_consumption\":${var:daily_consumption}}"
    },
    {
      "action_type": "webhook",
      "url": "https://erp.company.com/api/purchasing/create-requisition",
      "method": "POST",
      "message": "{\"item\":\"FEED-001\",\"quantity\":5000,\"unit\":\"kg\",\"priority\":\"urgent\",\"reason\":\"auto-reorder\",\"site\":\"${config:site_name}\"}",
      "condition": {
        "all": [
          {"sensor": "${var:days_remaining}", "operator": "less_than", "value": 5},
          {"sensor": "${var:silo_level}", "operator": "less_than", "value": 20}
        ]
      }
    },
    {
      "action_type": "alert",
      "level": "warning",
      "message": "Feed inventory low: ${var:days_remaining} days remaining, auto-reorder triggered",
      "condition": {
        "sensor": "${var:days_remaining}",
        "operator": "less_than",
        "value": 5
      }
    }
  ]
}
```

---

## Summary: Edge vs SCADA Capabilities

| Capability | SCADA | Edge | Benefit |
|------------|:-----:|:----:|---------|
| Real-time control | ✓ | ✗ | SCADA for safety-critical |
| PID loops | ✓ | ✗ | SCADA for <100ms response |
| Weather API integration | ✗ | ✓ | Predictive adjustments |
| Electricity spot prices | ✗ | ✓ | 20-30% energy savings |
| Multi-site comparison | ✗ | ✓ | Best practice sharing |
| Compliance reporting | ✗ | ✓ | Automated submissions |
| Slack/Teams alerts | ✗ | ✓ | Instant notifications |
| ERP integration | ✗ | ✓ | Supply chain automation |
| Predictive analytics | ✗ | ✓ | Harvest optimization |
| Recipe updates from cloud | ✗ | ✓ | Centralized management |

**The Principle**: Let PLC/SCADA do what it does best (deterministic control), let Edge do what it does best (connectivity and intelligence).

---

*Document: Edge Capabilities Beyond SCADA v1.2.4*
