import { NextRequest } from 'next/server';
import { verifySessionToken } from '@/lib/backend/auth';
import { UnauthorizedError, ForbiddenError } from '@/lib/backend/errors';

/**
 * Authenticated request shape used by protected routes.
 */
export interface AuthenticatedRequest extends NextRequest {
    user: {
        address: string;
        csrfToken?: string;
    };
}

/**
 * Require a valid session cookie and attach `user` to the request.
 */
export function requireAuth(req: NextRequest): AuthenticatedRequest {
    const sessionToken = req.cookies.get('session')?.value;
    if (!sessionToken) {
        throw new UnauthorizedError('No session token provided');
    }

    const verification = verifySessionToken(sessionToken);
    if (!verification.valid || !verification.address) {
        throw new UnauthorizedError(verification.error || 'Invalid session token');
    }

    const authenticatedReq = req as AuthenticatedRequest;
    authenticatedReq.user = { address: verification.address, csrfToken: verification.csrfToken };
    return authenticatedReq;
}

/**
 * Require administrative privileges. Admins may authenticate using the
 * `Authorization: Bearer <token>` header where the token must match
 * `COMMITLABS_ADMIN_SECRET`. Returns a minimal authenticated object used
 * by admin-only routes.
 */
export function requireAdmin(req: NextRequest): { address: string } {
    const adminSecret = process.env.COMMITLABS_ADMIN_SECRET ?? '';
    if (!adminSecret) {
        throw new ForbiddenError('Admin access is not configured.');
    }

    const header = req.headers.get('authorization') ?? '';
    const match = header.match(/^Bearer\s+(.+)$/i);
    const token = match ? match[1].trim() : '';
    if (!token || token !== adminSecret) {
        throw new ForbiddenError('Invalid or missing admin token.');
    }

    const address = process.env.COMMITLABS_ADMIN_ADDRESS ?? 'admin';
    return { address };
}

/**
 * Validate CSRF token for state-changing requests.
 */
export function validateCsrfToken(req: NextRequest, expectedCsrfToken: string): void {
    const method = req.method?.toUpperCase?.() ?? '';
    if (!['POST', 'PUT', 'PATCH', 'DELETE'].includes(method)) return;

    const providedCsrfToken = req.headers.get('x-csrf-token');
    if (!providedCsrfToken) throw new UnauthorizedError('CSRF token required for state-changing requests');
    if (providedCsrfToken !== expectedCsrfToken) throw new UnauthorizedError('Invalid CSRF token');
}

export function validateOrigin(req: NextRequest): void {
    const origin = req.headers.get('origin');
    const host = req.headers.get('host');
    const referer = req.headers.get('referer');

    if (!origin && !referer) return;
    if (origin && host) {
        const originHost = new URL(origin).host;
        if (originHost !== host) throw new UnauthorizedError('Cross-origin request not allowed');
    }
    if (referer && host && !origin) {
        const refererHost = new URL(referer).host;
        if (refererHost !== host) throw new UnauthorizedError('Cross-origin request not allowed');
    }
}
