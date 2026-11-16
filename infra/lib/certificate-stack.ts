import * as cdk from 'aws-cdk-lib';
import * as acm from 'aws-cdk-lib/aws-certificatemanager';
import * as route53 from 'aws-cdk-lib/aws-route53';
import type { Construct } from 'constructs';
import type { DomainConfig } from './config';

export interface CertificateStackProps extends cdk.StackProps {
  domain: DomainConfig;
}

/**
 * The CloudFront certificate. CloudFront reads certificates only from us-east-1, so
 * this small stack lives there and the main stack reads it through a cross-Region
 * reference. The hosted zone is passed by ID, so synthesis needs no AWS lookup.
 */
export class CertificateStack extends cdk.Stack {
  public readonly certificate: acm.ICertificate;

  constructor(scope: Construct, id: string, props: CertificateStackProps) {
    super(scope, id, { ...props, crossRegionReferences: true });
    const zone = route53.HostedZone.fromHostedZoneAttributes(this, 'Zone', {
      hostedZoneId: props.domain.hostedZoneId,
      zoneName: props.domain.hostedZoneName,
    });
    this.certificate = new acm.Certificate(this, 'Certificate', {
      domainName: props.domain.domainName,
      validation: acm.CertificateValidation.fromDns(zone),
    });
  }
}
