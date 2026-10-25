# zubspace-notifier. The build context is this directory:
#
#   docker build -t zubspace-notifier notifier/
#
# The check stage type-checks and runs the tests, so a broken notifier never
# becomes an image. The runtime image holds only Node and the sources; Node
# runs TypeScript directly, and the service has no runtime dependencies. It
# holds no config either: mount the real one (it carries the Slack webhook)
# at /app/etc/notifier.json. .dockerignore keeps a local etc/notifier.json out
# of the build context.
FROM docker.io/library/node:26.12.1-alpine3.24 AS check
WORKDIR /app
RUN npm install --global pnpm@12.14.0
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY tsconfig.json ./
COPY src ./src
COPY etc ./etc
RUN pnpm check && pnpm test && rm src/*.test.ts

FROM docker.io/library/node:26.12.1-alpine3.24
WORKDIR /app
COPY package.json ./
COPY --from=check /app/src ./src
USER node
# NOTIFIER_NTFY_TOPIC and NOTIFIER_SLACK_WEBHOOK_URL override the config's secret.
ENTRYPOINT ["node", "src/main.ts"]
CMD ["--config", "etc/notifier.json"]
