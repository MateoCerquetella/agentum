import type { HostedReviewInfo, HostedReviewProvider } from '@/shared/hosted-review'

export function hostedReviewCreationCopy(provider: HostedReviewProvider | null | undefined): {
  shortLabel: 'PR' | 'MR'
  reviewLabel: 'pull request' | 'merge request'
  titleLabel: 'Pull Request' | 'Merge Request'
  providerName: 'GitHub' | 'GitLab'
} {
  return provider === 'gitlab'
    ? {
        shortLabel: 'MR',
        reviewLabel: 'merge request',
        titleLabel: 'Merge Request',
        providerName: 'GitLab'
      }
    : {
        shortLabel: 'PR',
        reviewLabel: 'pull request',
        titleLabel: 'Pull Request',
        providerName: 'GitHub'
      }
}

export function hostedReviewStateClass(review: HostedReviewInfo): string {
  if (review.state === 'merged') {
    return 'text-purple-500/80'
  }
  if (review.state === 'open') {
    return 'text-emerald-500/80'
  }
  if (review.state === 'closed') {
    return 'text-muted-foreground/60'
  }
  return 'text-muted-foreground/50'
}

export function hostedReviewLabel(review: HostedReviewInfo): string {
  return `${review.provider === 'gitlab' ? 'MR' : 'PR'} #${review.number}`
}
