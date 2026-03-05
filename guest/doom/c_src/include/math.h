#ifndef _MATH_H
#define _MATH_H

/* Mathematical constants */
#define M_PI     3.14159265358979323846
#define M_PI_2   1.57079632679489661923
#define M_PI_4   0.78539816339744830962
#define M_E      2.71828182845904523536
#define M_SQRT2  1.41421356237309504880
#define M_LN2    0.69314718055994530942

/* Trigonometric functions (double precision) */
double sin(double x);
double cos(double x);
double tan(double x);
double asin(double x);
double acos(double x);
double atan(double x);
double atan2(double y, double x);

/* Exponential and logarithmic */
double exp(double x);
double log(double x);
double log10(double x);
double pow(double base, double exp);
double sqrt(double x);

/* Rounding */
double floor(double x);
double ceil(double x);

/* Absolute value */
double fabs(double x);

/* Float variants */
float  sinf(float x);
float  cosf(float x);
float  sqrtf(float x);
float  fabsf(float x);
float  floorf(float x);
float  ceilf(float x);
float  powf(float base, float exp);
float  atan2f(float y, float x);

/* Floating-point remainder */
double fmod(double x, double y);

#endif /* _MATH_H */
