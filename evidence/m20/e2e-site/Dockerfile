FROM docker.io/library/nginx:1.27-alpine
COPY index.html /usr/share/nginx/html/index.html
# stage_build_context writes files 0600; nginx reads them as its own user
RUN chmod -R a+rX /usr/share/nginx/html
