FROM node:12 as build-deps

# install tools and dependencies
RUN set -eux; \
	apt-get install -y git

# clone UI repo
RUN cd /usr/src/ && git clone https://github.com/paritytech/bridge-ui.git
WORKDIR /usr/src/bridge-ui
RUN yarn
ARG SUBSTRATE_PROVIDER
ARG ETHEREUM_PROVIDER
ARG EXPECTED_ETHEREUM_NETWORK_ID

ENV SUBSTRATE_PROVIDER $SUBSTRATE_PROVIDER
ENV ETHEREUM_PROVIDER $ETHEREUM_PROVIDER
ENV EXPECTED_ETHEREUM_NETWORK_ID $EXPECTED_ETHEREUM_NETWORK_ID

RUN yarn build:docker

# Stage 2 - the production environment
FROM nginx:1.12
COPY --from=build-deps /usr/src/bridge-ui/nginx/*.conf /etc/nginx/conf.d/
COPY --from=build-deps /usr/src/bridge-ui/dist /usr/share/nginx/html
EXPOSE 80
CMD ["nginx", "-g", "daemon off;"]
