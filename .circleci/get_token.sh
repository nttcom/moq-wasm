#!/usr/bin/env bash

base64url() {
    openssl enc -base64 -A | tr '+/' '-_' | tr -d '='
}

sign() {
    key="$(echo ${GITHUB_APPS_PEM_BASE64} | base64 --decode)"
    openssl dgst -binary -sha256 -sign <(printf '%s' "${key}")
}

header="$(printf '{"alg":"RS256","typ":"JWT"}' | base64url)"
now="$(date '+%s')"
iat="$((now - 60))"
exp="$((now + (3 * 60)))"
template='{"iss":"%s","iat":%s,"exp":%s}'
payload="$(printf "${template}" "${GITHUB_APPS_ID}" "${iat}" "${exp}" | base64url)"
signature="$(printf '%s' "${header}.${payload}" | sign | base64url)"
jwt="${header}.${payload}.${signature}"

installation_id="$(curl --location --silent --request GET \
    --url "https://api.github.com/repos/${CIRCLE_PROJECT_USERNAME}/${CIRCLE_PROJECT_REPONAME}/installation" \
    --header "Accept: application/vnd.github+json" \
    --header "X-GitHub-Api-Version: 2022-11-28" \
    --header "Authorization: Bearer ${jwt}" \
    | jq -r '.id'
)"

token="$(curl --location --silent --request POST \
    --url "https://api.github.com/app/installations/${installation_id}/access_tokens" \
    --header "Accept: application/vnd.github+json" \
    --header "X-GitHub-Api-Version: 2022-11-28" \
    --header "Authorization: Bearer ${jwt}" \
    | jq -r '.token'
)"

echo "${token}"