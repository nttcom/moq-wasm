FROM alpine:3.20

RUN apk add --no-cache nginx nginx-mod-rtmp nginx-mod-stream bash \
    && mkdir -p /run/nginx /var/log/nginx /var/media \
    && ln -sf /usr/lib/nginx/modules /etc/nginx/modules

EXPOSE 1935 8080

CMD ["nginx", "-g", "daemon off;"]
