export const dynamic = 'force-dynamic';
function missing() { return Response.json({ error: 'not_found' }, { status: 404 }); }
export const GET = missing;
export const POST = missing;
