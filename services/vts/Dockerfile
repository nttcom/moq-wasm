FROM node:22-alpine

WORKDIR /app
COPY services/vts/package.json services/vts/package-lock.json ./
RUN npm ci --omit=dev
COPY services/vts/src ./src
COPY services/vts/bin ./bin

ENV VTS_PORT=8081
ENV VTS_APPS_FILE=/etc/vts/apps.json
EXPOSE 8081
USER node
CMD ["node", "src/main.mjs"]
