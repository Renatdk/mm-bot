# Railway Web UI Setup

Service name suggestion: `mm-bot-web`

## Deploy settings

- Root Directory: repo root
- Builder: Dockerfile
- Dockerfile Path: `Dockerfile.web`

## Environment variables

- `NEXT_PUBLIC_API_BASE_URL` = `https://<api-domain>.up.railway.app`
- `PORT` is provided by Railway automatically

Important:

- `NEXT_PUBLIC_API_BASE_URL` is embedded during Next.js build.
- Set variable first, then trigger a new deploy.
- If you changed the variable after deploy, redeploy again.

## Post-deploy check

Open:

- `https://<web-domain>.up.railway.app`

Then verify:

1. Runs list is loading
2. Creating preset run works
3. Run details page shows logs/metrics/artifacts

## Important

In API service, include web domain in CORS:

`CORS_ALLOW_ORIGINS=http://localhost:3000,http://127.0.0.1:3000,https://<web-domain>.up.railway.app`
