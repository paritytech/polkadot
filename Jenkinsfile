@Library('jenkins-library')

String agentLabel             = 'docker-build-agent'
String registry               = 'docker.soramitsu.co.jp'
String dockerRegistryRWUserId = 'bot-sora2-rw'
String imageName              = 'docker.soramitsu.co.jp/sora2/polkadot-fearless'

properties([
    parameters([
        string(defaultValue: 'abb447c7', name: 'polkadotCommit', trim: true, description: 'It MUST be short version hash')
    ])
])

pipeline {
    options {
        buildDiscarder(logRotator(numToKeepStr: '20'))
        timestamps()
        disableConcurrentBuilds()
    }

    agent {
        label agentLabel
    }

    stages {
        stage('Build image') {
            steps{
                script {
                    gitNotify('polkadot-CI', 'PENDING', 'This commit is being built')
                    if ( polkadotCommit.length() != 8 ) {
                        error('You MUST use short version hash of commit!')
                    }
                    sh "docker build --build-arg POLKADOT_COMMIT=${polkadotCommit} -f housekeeping/docker/Dockerfile -t ${imageName}:${polkadotCommit} ."
                }
            }
        }
        stage('Push Image') {
            steps{
                script {
                    docker.withRegistry( 'https://' + registry, dockerRegistryRWUserId) {
                        sh """
                            docker push ${imageName}:${polkadotCommit}
                            docker tag ${imageName}:${polkadotCommit} ${imageName}:latest
                            docker push ${imageName}:latest
                        """
                    }
                }
            }
        }
    }
    post {
        cleanup { cleanWs() }
    }
}