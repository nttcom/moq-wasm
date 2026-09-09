FROM node:22-alpine

WORKDIR /app
COPY services/anon-issuer/package.json services/anon-issuer/package-lock.json ./
RUN npm ci --omit=dev
COPY services/anon-issuer/src ./src

ENV ANON_ISSUER_PORT=8080
EXPOSE 8080
USER node
CMD ["node", "src/main.mjs"]
