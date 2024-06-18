#!/usr/bin/env bash

cargo install cargo-llvm-cov
rustup component add llvm-tools-preview

artifact_link="https://output.circle-artifacts.com/output/job/${CIRCLE_WORKFLOW_JOB_ID}/artifacts/${CIRCLE_NODE_INDEX}/coverage-report/index.html"

cov_result=$(cargo llvm-cov | awk '/Filename/,0')
comment="\`\`\`shell-session
${cov_result}
\`\`\`

[Coverage report details](${artifact_link})
"

cargo llvm-cov report --html

apt update && apt install -y jq
GITHUB_APPS_TOKEN="$(bash .circleci/get_token.sh)"

github_comment_verion=6.0.4
pr_number=$(basename ${CIRCLE_PULL_REQUEST})

curl -L https://github.com/suzuki-shunsuke/github-comment/releases/download/v${github_comment_verion}/github-comment_${github_comment_verion}_linux_amd64.tar.gz -o linux_amd64.tar.gz
tar -zxvf linux_amd64.tar.gz
./github-comment hide --token ${GITHUB_APPS_TOKEN} --org ${CIRCLE_PROJECT_USERNAME} --repo ${CIRCLE_PROJECT_REPONAME} --pr ${pr_number} --condition 'Comment.HasMeta && Comment.Meta.TemplateKey == "default"'
./github-comment post --token ${GITHUB_APPS_TOKEN} --org ${CIRCLE_PROJECT_USERNAME} --repo ${CIRCLE_PROJECT_REPONAME} --pr ${pr_number} --template "${comment}"