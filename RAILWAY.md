# Railway Setup

Ниже минимальная схема для деплоя orchestration-слоя:

- Service `api`
- Service `worker`
- Service `postgresql` (Railway DB)
- Service `redis` (Railway Redis)

## 1) API service

- Root Directory: repo root
- Build Command: `cargo build -p api --release`
- Start Command: `cargo run -p api --release`

Environment variables:

- `DATABASE_URL` = connection string вашей Railway PostgreSQL
- `REDIS_URL` = connection string вашей Railway Redis
- `BIND_ADDR` = `0.0.0.0:$PORT`
- `RUST_LOG` = `api=info`

## 2) Worker service

- Root Directory: repo root
- Build Command: `cargo build -p worker --release`
- Start Command: `cargo run -p worker --release`

Environment variables:

- `DATABASE_URL` = connection string вашей Railway PostgreSQL
- `REDIS_URL` = connection string вашей Railway Redis
- `WORKSPACE_ROOT` = `/app`
- `RUST_LOG` = `worker=info`

## 3) Проверка

После деплоя API:

- `GET /health` -> `{ "ok": true }`
- `POST /runs` создаёт задачу и кладёт в Redis queue
- Worker подхватывает run и пишет логи в `run_events`

## 4) Пример POST /runs

```json
{
  "name": "MM MTF Jan-Feb",
  "kind": "backtest_mm_mtf",
  "cli_args": [
    "--symbol", "ETHUSDT",
    "--htf-interval", "5",
    "--ltf-interval", "1",
    "--start", "2026-01-01",
    "--end", "2026-02-10"
  ]
}
```
