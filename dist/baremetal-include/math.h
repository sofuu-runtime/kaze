/* bare-metal stub: the declarations an emitted Kaze program needs from <math.h>.
 *
 * The develop-host build links the platform libm; a board has none, so these are
 * implemented in kaze_baremetal.c. They are ordinary double routines, not chip-specific. */
#ifndef KAZE_BAREMETAL_MATH_H
#define KAZE_BAREMETAL_MATH_H

#ifndef INFINITY
#define INFINITY (1.0 / 0.0)
#endif
#define isnan(x) ((x) != (x))
#define isinf(x) (((x) == (1.0 / 0.0)) || ((x) == (-1.0 / 0.0)))

double fabs(double x);
double floor(double x);
double fmod(double x, double y);
double sqrt(double x);
double sin(double x);
double cos(double x);
double tan(double x);
double asin(double x);
double acos(double x);
double atan2(double y, double x);
double exp(double x);
double log(double x);
double pow(double base, double exponent);
double log10(double x);
double atan(double x);
double ceil(double x);
double round(double x);
double trunc(double x);
double fmin(double a, double b);
double fmax(double a, double b);

#endif
